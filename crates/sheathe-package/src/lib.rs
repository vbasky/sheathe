//! End-to-end VOD packaging pipeline for the sheathe packager.
//!
//! [`package`] takes one or more input media files (MP4, MPEG-TS, WebM/Matroska,
//! raw elementary streams, WebVTT/TTML) and writes CMAF init + media segments plus
//! DASH and/or HLS manifests into an output directory — the library form of the
//! `sheathe package` CLI command. Multiple inputs form an ABR ladder (each input's
//! track(s) become separate renditions sharing one manifest).
//!
//! The `sheathe-cli` `package` command is a thin wrapper over [`package`].

use anyhow::{Context, Result};
use sheathe_core::Sample;
use sheathe_core::{MediaKind, Scaled, StreamInfo};
use sheathe_crypto::{ContentKey, Scheme};
use sheathe_dash::{Manifest, Protection, Representation};
use sheathe_es::{EsDemuxer, is_mp4};
use sheathe_hls::{KeyInfo, SegmentRef, Variant, master_playlist, media_playlist};
use sheathe_mp4::{
    Encryption, Fragmenter, Mp4Demuxer, SegmentPolicy, Track, write_init_segment,
    write_media_segment,
};
use sheathe_ts::{TsDemuxer, packet::PACKET_SIZE};
use std::fs;
use std::path::{Path, PathBuf};

pub use sheathe_crypto::{ProtectionSystem as DrmSystem, Scheme as EncScheme};

/// Options controlling a [`package`] run.
#[derive(Debug, Clone)]
pub struct PackageOptions {
    /// Directory to write segments and manifests into (created if missing).
    pub out_dir: PathBuf,
    /// Target segment duration in seconds (segments cut on keyframes).
    pub segment_duration: f64,
    /// Emit a DASH manifest (`manifest.mpd`).
    pub dash: bool,
    /// Emit HLS playlists (`master.m3u8` + per-track media playlists).
    pub hls: bool,
    /// Content encryption; `None` produces clear output.
    pub encryption: Option<EncryptionSpec>,
}

impl Default for PackageOptions {
    fn default() -> Self {
        Self {
            out_dir: PathBuf::from("out"),
            segment_duration: 6.0,
            dash: true,
            hls: true,
            encryption: None,
        }
    }
}

/// Raw-key content encryption for a [`package`] run.
#[derive(Debug, Clone)]
pub struct EncryptionSpec {
    /// 16-byte key id.
    pub kid: [u8; 16],
    /// 16-byte content key.
    pub key: [u8; 16],
    /// Common Encryption scheme (`cenc`, `cens`, `cbc1`, `cbcs`).
    pub scheme: EncScheme,
    /// Key-delivery URI written into the HLS `#EXT-X-KEY` tag.
    pub key_uri: String,
    /// DRM systems to emit `pssh` boxes for.
    pub systems: Vec<DrmSystem>,
    /// Key-rotation crypto-period duration in seconds, or `None` for a single key.
    pub crypto_period_seconds: Option<f64>,
}

/// What a [`package`] run produced.
#[derive(Debug, Clone)]
pub struct PackageOutput {
    /// The output directory (echoes [`PackageOptions::out_dir`]).
    pub out_dir: PathBuf,
    /// Path to `manifest.mpd` when `dash` was requested.
    pub dash_manifest: Option<PathBuf>,
    /// Path to `master.m3u8` when `hls` was requested.
    pub hls_master: Option<PathBuf>,
    /// Every CMAF init segment written (one per rendition).
    pub init_segments: Vec<PathBuf>,
    /// Every CMAF media segment written.
    pub media_segments: Vec<PathBuf>,
    /// Longest rendition duration in seconds.
    pub duration_seconds: f64,
    /// Number of renditions produced (across all inputs/tracks).
    pub renditions: usize,
}

/// Package one or more inputs into CMAF segments + DASH and/or HLS manifests under
/// `opts.out_dir`. Each input's track(s) become separate renditions sharing one
/// manifest — several video inputs form an ABR ladder.
pub fn package(inputs: &[PathBuf], opts: &PackageOptions) -> Result<PackageOutput> {
    anyhow::ensure!(!inputs.is_empty(), "package: at least one input is required");

    let out_dir = &opts.out_dir;
    fs::create_dir_all(out_dir).with_context(|| format!("creating {}/", out_dir.display()))?;

    let encryption: Option<Encryption> = opts.encryption.as_ref().map(build_encryption);

    // HLS `#EXT-X-KEY` signalling for encrypted output.
    let hls_key = opts.encryption.as_ref().map(|spec| KeyInfo {
        // HLS fMP4 maps the CBC schemes to SAMPLE-AES and the CTR schemes to
        // SAMPLE-AES-CTR.
        method: match spec.scheme {
            Scheme::Cbcs | Scheme::Cbc1 => "SAMPLE-AES",
            _ => "SAMPLE-AES-CTR",
        }
        .to_string(),
        key_format: "urn:mpeg:dash:mp4protection:2011".to_string(),
        uri: spec.key_uri.clone(),
    });

    let datas: Vec<Vec<u8>> = inputs
        .iter()
        .map(|p| fs::read(p).with_context(|| format!("reading {}", p.display())))
        .collect::<Result<_>>()?;
    let loaded: Vec<LoadedInput> = datas
        .iter()
        .zip(inputs)
        .map(|(d, p)| load_input(&p.to_string_lossy(), d))
        .collect::<Result<_>>()?;

    let policy = SegmentPolicy { target_seconds: opts.segment_duration, keyframes_only: true };
    let mut dash_reps = Vec::new();
    let mut hls_variants = Vec::new();
    let mut init_segments = Vec::new();
    let mut media_segments = Vec::new();
    let mut total_seconds = 0.0_f64;
    let mut rep = 0usize; // global rendition index across all inputs/tracks

    for input in &loaded {
        for lt in &input.tracks {
            let track = &lt.track;
            let samples = &lt.samples;
            let mut frag = Fragmenter::new(track.info.clone(), policy);
            for s in samples.iter().cloned() {
                frag.push(s)?;
            }
            let segments = frag.finish();
            let ts = track.info.timescale;

            // Init segment.
            let init_name = format!("init_{rep}.mp4");
            fs::write(out_dir.join(&init_name), write_init_segment(track, encryption.as_ref()))
                .with_context(|| format!("writing {init_name}"))?;
            init_segments.push(out_dir.join(&init_name));

            // Media segments.
            let mut durations = Vec::with_capacity(segments.len());
            let mut hls_segs = Vec::with_capacity(segments.len());
            let mut sample_index = 0u64;
            for (n, seg) in segments.iter().enumerate() {
                let seg_name = format!("seg_{rep}_{}.m4s", n + 1);
                let data = write_media_segment(
                    track,
                    (n + 1) as u32,
                    seg,
                    sample_index,
                    encryption.as_ref(),
                );
                fs::write(out_dir.join(&seg_name), data)
                    .with_context(|| format!("writing {seg_name}"))?;
                media_segments.push(out_dir.join(&seg_name));
                sample_index += seg.samples.len() as u64;
                durations.push(seg.duration_ticks);
                hls_segs.push(SegmentRef {
                    duration: Scaled::new(seg.duration_ticks, ts).seconds(),
                    uri: seg_name,
                });
            }

            let track_total: u64 = segments.iter().map(|s| s.duration_ticks).sum();
            let track_seconds = Scaled::new(track_total, ts).seconds();
            total_seconds = total_seconds.max(track_seconds);

            dash_reps.push(Representation {
                id: rep.to_string(),
                stream: track.info.clone(),
                init: init_name.clone(),
                media: format!("seg_{rep}_$Number$.m4s"),
                timescale: ts.0,
                segment_durations: durations,
            });

            if opts.hls {
                let media_name = format!("media_{rep}.m3u8");
                fs::write(
                    out_dir.join(&media_name),
                    media_playlist(&init_name, &hls_segs, hls_key.as_ref()),
                )
                .with_context(|| format!("writing {media_name}"))?;
                hls_variants.push(Variant { stream: track.info.clone(), playlist_uri: media_name });
            }

            rep += 1;
        }
    }

    let mut dash_manifest = None;
    if opts.dash {
        let protection = opts.encryption.as_ref().map(|spec| Protection {
            scheme: scheme_str(spec.scheme).to_string(),
            default_kid: spec.kid,
        });
        let mpd =
            Manifest { duration_seconds: total_seconds, representations: dash_reps, protection }
                .to_xml();
        let path = out_dir.join("manifest.mpd");
        fs::write(&path, mpd).context("writing manifest.mpd")?;
        dash_manifest = Some(path);
    }

    let mut hls_master = None;
    if opts.hls {
        let path = out_dir.join("master.m3u8");
        fs::write(&path, master_playlist(&hls_variants)).context("writing master.m3u8")?;
        hls_master = Some(path);
    }

    Ok(PackageOutput {
        out_dir: out_dir.clone(),
        dash_manifest,
        hls_master,
        init_segments,
        media_segments,
        duration_seconds: total_seconds,
        renditions: rep,
    })
}

/// Human-readable name for a [`Scheme`] (matches the CLI/DASH spelling).
pub fn scheme_str(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Cenc => "cenc",
        Scheme::Cens => "cens",
        Scheme::Cbc1 => "cbc1",
        Scheme::Cbcs => "cbcs",
    }
}

fn build_encryption(spec: &EncryptionSpec) -> Encryption {
    // A fixed, asset-wide constant IV for cbcs (cenc derives per-sample IVs and
    // ignores this).
    let constant_iv = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    Encryption {
        scheme: spec.scheme,
        key: ContentKey { kid: spec.kid, key: spec.key },
        constant_iv,
        systems: spec.systems.clone(),
        crypto_period_seconds: spec.crypto_period_seconds,
    }
}

/// One stream discovered by [`probe`].
#[derive(Debug, Clone)]
pub struct ProbeStream {
    /// In-container track id.
    pub track_id: u32,
    /// Stream metadata (codec, resolution, timescale, …).
    pub info: StreamInfo,
    /// Number of coded samples in the track.
    pub sample_count: usize,
}

/// Result of [`probe`]: the detected container and its streams.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    /// Detected container format (e.g. `"MP4"`, `"MPEG-TS"`).
    pub format: &'static str,
    /// Input size in bytes.
    pub size_bytes: usize,
    /// The streams the demuxer found.
    pub streams: Vec<ProbeStream>,
}

/// Inspect an input and report the container + streams sheathe detects, without
/// writing anything. The library form of the `sheathe probe` CLI command.
pub fn probe(input: &Path) -> Result<ProbeReport> {
    let bytes = fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let loaded = load_input(&input.to_string_lossy(), &bytes)?;
    let streams = loaded
        .tracks
        .iter()
        .map(|lt| ProbeStream {
            track_id: lt.track.track_id,
            info: lt.track.info.clone(),
            sample_count: lt.samples.len(),
        })
        .collect();
    Ok(ProbeReport { format: loaded.format, size_bytes: bytes.len(), streams })
}

/// A loaded input with pre-extracted tracks and samples.
struct LoadedInput {
    format: &'static str,
    tracks: Vec<LoadedTrack>,
}

struct LoadedTrack {
    track: Track,
    samples: Vec<Sample>,
}

/// Detect MPEG-TS by 0x47 sync bytes at 188-byte intervals.
fn is_transport_stream(data: &[u8]) -> bool {
    if data.len() < PACKET_SIZE * 3 {
        return false;
    }
    (0..3).all(|i| data[i * PACKET_SIZE] == 0x47)
}

/// Extract CEA-608 captions from an Annex B H.264/H.265 video track and append
/// them as a WebVTT text track. A no-op when no captions are present.
fn append_captions(tracks: &mut Vec<LoadedTrack>) {
    let Some(vid) = tracks.iter().find(|t| {
        t.track.info.kind == MediaKind::Video
            && matches!(t.track.info.codec, sheathe_core::Codec::H264 | sheathe_core::Codec::H265)
    }) else {
        return;
    };
    let hevc = vid.track.info.codec == sheathe_core::Codec::H265;
    let samples: Vec<(u64, &[u8])> =
        vid.samples.iter().map(|s| (s.pts, s.data.as_slice())).collect();
    for text in sheathe_text::extract_captions(&samples, hevc) {
        let id = tracks.len() as u32 + 1;
        tracks.push(LoadedTrack {
            track: Track::from_sample_entry(
                text.info.clone(),
                id,
                text.sample_entry.clone(),
                &text.samples,
            ),
            samples: text.samples.clone(),
        });
    }
}

fn load_input(path: &str, data: &[u8]) -> Result<LoadedInput> {
    if is_transport_stream(data) {
        let demux = TsDemuxer::parse(data).with_context(|| format!("parsing MPEG-TS {path}"))?;
        let mut tracks: Vec<LoadedTrack> = demux
            .tracks()
            .iter()
            .enumerate()
            .map(|(i, t)| LoadedTrack {
                track: Track::from_sample_entry(
                    t.info.clone(),
                    (i + 1) as u32,
                    t.sample_entry.clone(),
                    &t.samples,
                ),
                samples: t.samples.clone(),
            })
            .collect();
        append_captions(&mut tracks);
        return Ok(LoadedInput { format: "MPEG-TS", tracks });
    }

    if is_mp4(data) {
        return load_mp4(path, data);
    }

    if sheathe_mkv::is_webm(data) {
        let demux =
            sheathe_mkv::MkvDemuxer::parse(data).with_context(|| format!("parsing WebM {path}"))?;
        let tracks = demux
            .tracks()
            .iter()
            .enumerate()
            .map(|(i, t)| LoadedTrack {
                track: Track::from_sample_entry(
                    t.info.clone(),
                    (i + 1) as u32,
                    t.sample_entry.clone(),
                    &t.samples,
                ),
                samples: t.samples.clone(),
            })
            .collect();
        return Ok(LoadedInput { format: "WebM", tracks });
    }

    if sheathe_text::is_webvtt(path, data) {
        let text = std::str::from_utf8(data)
            .with_context(|| format!("WebVTT {path} is not valid UTF-8"))?;
        let t = sheathe_text::webvtt(text).with_context(|| format!("parsing WebVTT {path}"))?;
        let tracks = vec![LoadedTrack {
            track: Track::from_sample_entry(t.info.clone(), 1, t.sample_entry.clone(), &t.samples),
            samples: t.samples.clone(),
        }];
        return Ok(LoadedInput { format: "WebVTT", tracks });
    }

    if sheathe_text::is_ttml(data) {
        let text =
            std::str::from_utf8(data).with_context(|| format!("TTML {path} is not valid UTF-8"))?;
        let t = sheathe_text::ttml(text).with_context(|| format!("parsing TTML {path}"))?;
        let tracks = vec![LoadedTrack {
            track: Track::from_sample_entry(t.info.clone(), 1, t.sample_entry.clone(), &t.samples),
            samples: t.samples.clone(),
        }];
        return Ok(LoadedInput { format: "TTML", tracks });
    }

    if sheathe_es::detect(path, data).is_some() {
        let demux = EsDemuxer::parse_auto(path, data)
            .with_context(|| format!("parsing elementary stream {path}"))?;
        let t = demux.track();
        let mut tracks = vec![LoadedTrack {
            track: Track::from_sample_entry(t.info.clone(), 1, t.sample_entry.clone(), &t.samples),
            samples: t.samples.clone(),
        }];
        append_captions(&mut tracks);
        return Ok(LoadedInput { format: "elementary", tracks });
    }

    load_mp4(path, data)
}

fn load_mp4(path: &str, data: &[u8]) -> Result<LoadedInput> {
    let demux = Mp4Demuxer::parse(data).with_context(|| format!("parsing MP4 {path}"))?;
    let mut tracks = Vec::new();
    for (i, t) in demux.tracks().iter().enumerate() {
        tracks.push(LoadedTrack {
            track: t.clone(),
            samples: demux.samples(i).with_context(|| format!("reading samples for track {i}"))?,
        });
    }
    Ok(LoadedInput { format: "MP4", tracks })
}

/// One-line human description of a stream (re-exported for CLI/probe use).
pub fn describe(info: &StreamInfo) -> String {
    let kind = match info.kind {
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::Text => "text",
    };
    let mut s = format!("{kind} {}", info.rfc6381());
    if let Some((w, h)) = info.resolution {
        s.push_str(&format!(" {w}x{h}"));
    }
    if let Some(rate) = info.sample_rate {
        s.push_str(&format!(" {rate}Hz"));
    }
    if let Some(br) = info.bitrate {
        s.push_str(&format!(" ~{}kbps", br / 1000));
    }
    s
}

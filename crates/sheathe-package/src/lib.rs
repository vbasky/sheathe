//! End-to-end packaging pipeline for the sheathe packager.
//!
//! [`package`] takes one or more input media files (MP4, MPEG-TS, WebM/Matroska,
//! raw elementary streams, WebVTT/TTML) and writes CMAF init + media segments plus
//! DASH and/or HLS manifests into an output directory — the library form of the
//! `sheathe package` CLI command.
//!
//! Multiple inputs form an ABR ladder by default (each input's track(s) become
//! separate renditions sharing one period). With [`PackageOptions::multi_period`]
//! each input becomes a successive DASH Period instead.
//!
//! Phase 4 (live & advanced manifests) is controlled via
//! [`PresentationMode`], live window size, trick-play, low-latency parts, and
//! SCTE-35 markers.

mod scte35;

use anyhow::{Context, Result};
use sheathe_core::Sample;
use sheathe_core::{MediaKind, Scaled, StreamInfo};
use sheathe_crypto::{ContentKey, Scheme};
use sheathe_dash::{
    DashEvent, EventStream, Manifest, MpdType, Period, Protection, Representation, UtcTiming,
};
use sheathe_es::{EsDemuxer, is_mp4};
use sheathe_hls::{
    DateRange, KeyInfo, MediaPlaylist, PartialSegment, SegmentRef, Variant, iframe_playlist,
    master_playlist,
};
use sheathe_mp4::{
    Encryption, Fragmenter, Mp4Demuxer, Segment, SegmentPolicy, Track, write_init_segment,
    write_media_segment,
};
use sheathe_ts::{TsDemuxer, packet::PACKET_SIZE};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub use scte35::{Scte35Marker, build_splice_insert, to_base64, to_hex_0x};
pub use sheathe_crypto::{ProtectionSystem as DrmSystem, Scheme as EncScheme};

/// How the presentation should be signalled in DASH/HLS manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresentationMode {
    /// Finished VOD asset (`type=static`, `#EXT-X-PLAYLIST-TYPE:VOD` + ENDLIST).
    #[default]
    Vod,
    /// Growing event (`type=dynamic` without a sliding window, EVENT playlist).
    Event,
    /// Live sliding window (`type=dynamic`, no ENDLIST, `#EXT-X-MEDIA-SEQUENCE`).
    Live,
}

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
    /// VOD / EVENT / Live presentation mode.
    pub presentation: PresentationMode,
    /// Live window size in segments. `None` keeps every segment (EVENT/VOD) or
    /// defaults to 3 for Live.
    pub live_window_segments: Option<usize>,
    /// Treat each input as a successive DASH Period (and separate HLS master
    /// variant group) instead of an ABR ladder in one period.
    pub multi_period: bool,
    /// Emit trick-play (I-frame) tracks for video renditions.
    pub trick_play: bool,
    /// Low-latency packaging: split media segments into parts.
    pub low_latency: bool,
    /// Part target duration in seconds when `low_latency` is set (default: 1s).
    pub part_duration: Option<f64>,
    /// Wall-clock availability start (ISO-8601). Defaults to "now" for live/event.
    pub availability_start_time: Option<String>,
    /// SCTE-35 ad markers to inject into manifests.
    pub scte35_markers: Vec<Scte35Marker>,
}

impl Default for PackageOptions {
    fn default() -> Self {
        Self {
            out_dir: PathBuf::from("out"),
            segment_duration: 6.0,
            dash: true,
            hls: true,
            encryption: None,
            presentation: PresentationMode::Vod,
            live_window_segments: None,
            multi_period: false,
            trick_play: false,
            low_latency: false,
            part_duration: None,
            availability_start_time: None,
            scte35_markers: Vec::new(),
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

/// Intermediate state for one packaged rendition (one track of one input).
struct PackagedRendition {
    stream: StreamInfo,
    init_name: String,
    media_template: String,
    timescale: u32,
    /// Full list of segment durations (timescale ticks), pre-window.
    all_durations: Vec<u64>,
    /// Full list of HLS segment refs, pre-window.
    all_hls_segs: Vec<SegmentRef>,
    /// Period index this rendition belongs to (0 for ABR ladder).
    period_index: usize,
    /// Trick-play companion (I-frame only), if any.
    trick: Option<TrickRendition>,
}

struct TrickRendition {
    init_name: String,
    media_template: String,
    all_durations: Vec<u64>,
    all_hls_segs: Vec<SegmentRef>,
}

/// Package one or more inputs into CMAF segments + DASH and/or HLS manifests under
/// `opts.out_dir`.
pub fn package(inputs: &[PathBuf], opts: &PackageOptions) -> Result<PackageOutput> {
    anyhow::ensure!(!inputs.is_empty(), "package: at least one input is required");
    if opts.low_latency {
        anyhow::ensure!(
            opts.part_duration.unwrap_or(1.0) > 0.0,
            "package: part_duration must be positive when low_latency is set"
        );
    }

    let out_dir = &opts.out_dir;
    fs::create_dir_all(out_dir).with_context(|| format!("creating {}/", out_dir.display()))?;

    let encryption: Option<Encryption> = opts.encryption.as_ref().map(build_encryption);

    let hls_key = opts.encryption.as_ref().map(|spec| KeyInfo {
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
    let part_dur = opts.part_duration.unwrap_or(1.0);

    let mut renditions = Vec::new();
    let mut init_segments = Vec::new();
    let mut media_segments = Vec::new();
    let mut total_seconds = 0.0_f64;
    let mut rep = 0usize;
    // Per-input duration for multi-period starts.
    let mut period_durations: Vec<f64> = Vec::new();

    for (input_idx, input) in loaded.iter().enumerate() {
        let period_index = if opts.multi_period { input_idx } else { 0 };
        let mut input_seconds = 0.0_f64;

        for lt in &input.tracks {
            let track = &lt.track;
            let samples = &lt.samples;
            let mut frag = Fragmenter::new(track.info.clone(), policy);
            for s in samples.iter().cloned() {
                frag.push(s)?;
            }
            let segments = frag.finish();
            let ts = track.info.timescale;

            let init_name = format!("init_{rep}.mp4");
            fs::write(out_dir.join(&init_name), write_init_segment(track, encryption.as_ref()))
                .with_context(|| format!("writing {init_name}"))?;
            init_segments.push(out_dir.join(&init_name));

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

                let mut href = SegmentRef::new(
                    Scaled::new(seg.duration_ticks, ts).seconds(),
                    seg_name.clone(),
                );
                if opts.low_latency && track.info.kind == MediaKind::Video {
                    href.parts = write_ll_parts(
                        out_dir,
                        track,
                        rep,
                        n + 1,
                        seg,
                        sample_index - seg.samples.len() as u64,
                        encryption.as_ref(),
                        part_dur,
                        &mut media_segments,
                    )?;
                }
                hls_segs.push(href);
            }

            let track_total: u64 = segments.iter().map(|s| s.duration_ticks).sum();
            let track_seconds = Scaled::new(track_total, ts).seconds();
            total_seconds = total_seconds.max(track_seconds);
            input_seconds = input_seconds.max(track_seconds);

            let trick = if opts.trick_play && track.info.kind == MediaKind::Video {
                Some(write_trick_play(
                    out_dir,
                    track,
                    samples,
                    rep,
                    encryption.as_ref(),
                    &mut init_segments,
                    &mut media_segments,
                )?)
            } else {
                None
            };

            renditions.push(PackagedRendition {
                stream: track.info.clone(),
                init_name,
                media_template: format!("seg_{rep}_$Number$.m4s"),
                timescale: ts.0,
                all_durations: durations,
                all_hls_segs: hls_segs,
                period_index,
                trick,
            });
            rep += 1;
        }

        if opts.multi_period {
            period_durations.push(input_seconds);
        }
    }

    let window = resolve_window(opts, &renditions);
    let avail_start = opts.availability_start_time.clone().unwrap_or_else(iso8601_now);
    let publish_time = iso8601_now();

    let mut dash_manifest = None;
    if opts.dash {
        let mpd = build_dash_manifest(
            opts,
            &renditions,
            window,
            total_seconds,
            &period_durations,
            &avail_start,
            &publish_time,
        );
        let path = out_dir.join("manifest.mpd");
        fs::write(&path, mpd.to_xml()).context("writing manifest.mpd")?;
        dash_manifest = Some(path);
    }

    let mut hls_master = None;
    if opts.hls {
        let variants = write_hls_playlists(
            out_dir,
            opts,
            &renditions,
            window,
            hls_key.as_ref(),
            &avail_start,
        )?;
        let path = out_dir.join("master.m3u8");
        fs::write(&path, master_playlist(&variants)).context("writing master.m3u8")?;
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

fn resolve_window(opts: &PackageOptions, renditions: &[PackagedRendition]) -> Option<usize> {
    match opts.presentation {
        PresentationMode::Vod => None,
        PresentationMode::Event => opts.live_window_segments,
        PresentationMode::Live => {
            Some(opts.live_window_segments.unwrap_or_else(|| {
                // Default: last 3 segments, or whatever exists.
                let max_len = renditions.iter().map(|r| r.all_durations.len()).max().unwrap_or(0);
                max_len.clamp(1, 3)
            }))
        }
    }
}

/// Slice the trailing `window` segments (or the full list when `window` is None).
fn window_slice<T: Clone>(items: &[T], window: Option<usize>) -> (usize, Vec<T>) {
    match window {
        Some(w) if w < items.len() => {
            let start = items.len() - w;
            (start, items[start..].to_vec())
        }
        _ => (0, items.to_vec()),
    }
}

fn build_dash_manifest(
    opts: &PackageOptions,
    renditions: &[PackagedRendition],
    window: Option<usize>,
    total_seconds: f64,
    period_durations: &[f64],
    avail_start: &str,
    publish_time: &str,
) -> Manifest {
    let protection = opts.encryption.as_ref().map(|spec| Protection {
        scheme: scheme_str(spec.scheme).to_string(),
        default_kid: spec.kid,
    });

    let event_streams = scte35_event_streams(&opts.scte35_markers, 90_000);

    let num_periods = if opts.multi_period { period_durations.len().max(1) } else { 1 };

    let mut periods = Vec::with_capacity(num_periods);
    let mut period_start = 0.0_f64;
    for p in 0..num_periods {
        let reps_in_period: Vec<&PackagedRendition> =
            renditions.iter().filter(|r| r.period_index == p).collect();

        let mut representations = Vec::new();
        for r in &reps_in_period {
            let (start_idx, durs) = window_slice(&r.all_durations, window);
            let start_number = (start_idx as u32) + 1;
            let pto: u64 = r.all_durations[..start_idx].iter().sum();
            let ato =
                if opts.low_latency { Some(opts.part_duration.unwrap_or(1.0) * 3.0) } else { None };
            let rep_id = r.init_name[5..r.init_name.len() - 4].to_string();
            let mut rep = Representation::new(
                rep_id.clone(),
                r.stream.clone(),
                r.init_name.clone(),
                r.media_template.clone(),
                r.timescale,
                durs,
            );
            rep.start_number = start_number;
            rep.presentation_time_offset = pto;
            rep.availability_time_offset = ato;
            representations.push(rep);

            if let Some(trick) = &r.trick {
                let (t_start, t_durs) = window_slice(&trick.all_durations, window);
                let mut trep = Representation::new(
                    format!("{rep_id}_trick"),
                    r.stream.clone(),
                    trick.init_name.clone(),
                    trick.media_template.clone(),
                    r.timescale,
                    t_durs,
                );
                trep.start_number = t_start as u32 + 1;
                trep.presentation_time_offset = trick.all_durations[..t_start].iter().sum();
                trep.max_playout_rate = Some(8.0);
                representations.push(trep);
            }
        }

        let period_dur = if opts.multi_period {
            period_durations.get(p).copied()
        } else if opts.presentation == PresentationMode::Vod {
            Some(total_seconds)
        } else {
            None
        };

        periods.push(Period {
            id: format!("p{p}"),
            start_seconds: Some(period_start),
            duration_seconds: period_dur,
            representations,
            event_streams: if p == 0 { event_streams.clone() } else { Vec::new() },
        });
        if let Some(d) = period_dur {
            period_start += d;
        }
    }

    match opts.presentation {
        PresentationMode::Vod => Manifest {
            mpd_type: MpdType::Static,
            duration_seconds: Some(if opts.multi_period {
                period_durations.iter().sum()
            } else {
                total_seconds
            }),
            periods,
            protection,
            ..Manifest::default()
        },
        PresentationMode::Event | PresentationMode::Live => {
            let window_secs =
                window.map(|w| w as f64 * opts.segment_duration).unwrap_or(total_seconds);
            Manifest {
                mpd_type: MpdType::Dynamic,
                duration_seconds: None,
                availability_start_time: Some(avail_start.to_string()),
                publish_time: Some(publish_time.to_string()),
                minimum_update_period: Some(if opts.low_latency {
                    opts.part_duration.unwrap_or(1.0)
                } else {
                    opts.segment_duration.min(2.0)
                }),
                time_shift_buffer_depth: Some(window_secs.max(opts.segment_duration)),
                suggested_presentation_delay: Some((window_secs / 2.0).max(opts.segment_duration)),
                utc_timing: Some(UtcTiming::http_iso("https://time.akamai.com/?iso")),
                periods,
                protection,
            }
        }
    }
}

fn scte35_event_streams(markers: &[Scte35Marker], timescale: u32) -> Vec<EventStream> {
    if markers.is_empty() {
        return Vec::new();
    }
    let events: Vec<DashEvent> = markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let bytes = build_splice_insert(m);
            DashEvent {
                id: Some(format!("{}", m.event_id.unwrap_or(i as u32 + 1))),
                presentation_time: (m.time_seconds * f64::from(timescale)).round() as u64,
                duration: m
                    .break_duration_seconds
                    .map(|d| (d * f64::from(timescale)).round() as u64),
                message_data: Some(to_base64(&bytes)),
            }
        })
        .collect();
    vec![EventStream::scte35_bin(timescale, events)]
}

fn write_hls_playlists(
    out_dir: &Path,
    opts: &PackageOptions,
    renditions: &[PackagedRendition],
    window: Option<usize>,
    hls_key: Option<&KeyInfo>,
    avail_start: &str,
) -> Result<Vec<Variant>> {
    let mut variants = Vec::new();
    let dateranges = scte35_dateranges(&opts.scte35_markers, avail_start);

    for r in renditions {
        let (start_idx, segs) = window_slice(&r.all_hls_segs, window);
        let media_sequence = start_idx as u64;
        // init name is init_{id}.mp4 — extract id
        let id = &r.init_name[5..r.init_name.len() - 4];
        let media_name = format!("media_{id}.m3u8");

        let mut pl = match opts.presentation {
            PresentationMode::Vod => MediaPlaylist::vod(&r.init_name, segs, hls_key.cloned()),
            PresentationMode::Event => {
                MediaPlaylist::event(&r.init_name, segs, hls_key.cloned(), false)
            }
            PresentationMode::Live => {
                MediaPlaylist::live(&r.init_name, media_sequence, segs, hls_key.cloned())
            }
        };
        // Only signal LL-HLS tags when this rendition actually has parts
        // (video tracks under --low-latency).
        let has_parts = pl.segments.iter().any(|s| !s.parts.is_empty());
        if opts.low_latency && has_parts {
            pl.part_target = Some(opts.part_duration.unwrap_or(1.0));
            pl.part_hold_back = Some(opts.part_duration.unwrap_or(1.0) * 3.0);
            pl.can_block_reload = true;
            if let Some(last_part) = pl.segments.iter().rev().find_map(|s| s.parts.last()) {
                // Hint the next part URI pattern.
                let hint = last_part.uri.replace(".m4s", "") + ".next.m4s";
                pl.preload_hint = Some(("PART".into(), hint));
            }
        }
        if r.period_index == 0 {
            pl.dateranges = dateranges.clone();
        }
        // Tag first segment with program-date-time for live/event.
        if matches!(opts.presentation, PresentationMode::Live | PresentationMode::Event)
            && let Some(first) = pl.segments.first_mut()
        {
            first.program_date_time = Some(avail_start.to_string());
        }

        fs::write(out_dir.join(&media_name), pl.to_m3u8())
            .with_context(|| format!("writing {media_name}"))?;

        let iframe_uri = if let Some(trick) = &r.trick {
            let (_t0, tsegs) = window_slice(&trick.all_hls_segs, window);
            let iframe_name = format!("iframe_{id}.m3u8");
            fs::write(out_dir.join(&iframe_name), iframe_playlist(&trick.init_name, &tsegs))
                .with_context(|| format!("writing {iframe_name}"))?;
            Some(iframe_name)
        } else {
            None
        };

        variants.push(Variant {
            stream: r.stream.clone(),
            playlist_uri: media_name,
            iframe_playlist_uri: iframe_uri,
        });
    }
    Ok(variants)
}

fn scte35_dateranges(markers: &[Scte35Marker], avail_start: &str) -> Vec<DateRange> {
    markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let bytes = build_splice_insert(m);
            let hex = to_hex_0x(&bytes);
            let start = offset_iso8601(avail_start, m.time_seconds);
            let id = format!("scte35-{}", m.event_id.unwrap_or(i as u32 + 1));
            if m.out_of_network {
                let mut dr = DateRange::scte35_out(id, start, hex);
                dr.planned_duration = m.break_duration_seconds;
                dr
            } else {
                DateRange::scte35_in(id, start, hex)
            }
        })
        .collect()
}

/// Write keyframe-only trick-play segments for a video track.
fn write_trick_play(
    out_dir: &Path,
    track: &Track,
    samples: &[Sample],
    rep: usize,
    encryption: Option<&Encryption>,
    init_segments: &mut Vec<PathBuf>,
    media_segments: &mut Vec<PathBuf>,
) -> Result<TrickRendition> {
    let keyframes: Vec<Sample> =
        samples.iter().filter(|s| s.is_segment_boundary()).cloned().collect();
    anyhow::ensure!(!keyframes.is_empty(), "trick-play: video track has no keyframes");

    // One segment per keyframe so the I-frame playlist can seek freely.
    let policy = SegmentPolicy { target_seconds: 0.001, keyframes_only: true };
    let mut frag = Fragmenter::new(track.info.clone(), policy);
    for s in keyframes {
        frag.push(s)?;
    }
    let segments = frag.finish();
    let ts = track.info.timescale;

    let init_name = format!("init_{rep}_trick.mp4");
    fs::write(out_dir.join(&init_name), write_init_segment(track, encryption))
        .with_context(|| format!("writing {init_name}"))?;
    init_segments.push(out_dir.join(&init_name));

    let mut durations = Vec::new();
    let mut hls_segs = Vec::new();
    let mut sample_index = 0u64;
    for (n, seg) in segments.iter().enumerate() {
        let seg_name = format!("seg_{rep}_trick_{}.m4s", n + 1);
        let data = write_media_segment(track, (n + 1) as u32, seg, sample_index, encryption);
        fs::write(out_dir.join(&seg_name), data).with_context(|| format!("writing {seg_name}"))?;
        media_segments.push(out_dir.join(&seg_name));
        sample_index += seg.samples.len() as u64;
        durations.push(seg.duration_ticks);
        hls_segs.push(SegmentRef::new(Scaled::new(seg.duration_ticks, ts).seconds(), seg_name));
    }

    Ok(TrickRendition {
        init_name,
        media_template: format!("seg_{rep}_trick_$Number$.m4s"),
        all_durations: durations,
        all_hls_segs: hls_segs,
    })
}

/// Split a segment into low-latency parts by sample duration budget.
#[allow(clippy::too_many_arguments)]
fn write_ll_parts(
    out_dir: &Path,
    track: &Track,
    rep: usize,
    seg_number: usize,
    seg: &Segment,
    base_sample_index: u64,
    encryption: Option<&Encryption>,
    part_duration_secs: f64,
    media_segments: &mut Vec<PathBuf>,
) -> Result<Vec<PartialSegment>> {
    let ts = track.info.timescale.0.max(1) as f64;
    let part_ticks = (part_duration_secs * ts).round().max(1.0) as u64;
    let mut parts = Vec::new();
    let mut acc: Vec<Sample> = Vec::new();
    let mut acc_ticks = 0u64;
    let mut part_idx = 1usize;
    let mut sample_index = base_sample_index;
    let mut part_start_dts = seg.start_ticks;

    let flush = |acc: &mut Vec<Sample>,
                 acc_ticks: &mut u64,
                 part_idx: &mut usize,
                 sample_index: &mut u64,
                 part_start_dts: &mut u64,
                 parts: &mut Vec<PartialSegment>,
                 media_segments: &mut Vec<PathBuf>|
     -> Result<()> {
        if acc.is_empty() {
            return Ok(());
        }
        let independent = acc.first().is_some_and(|s| s.is_segment_boundary());
        let part_seg = Segment {
            start_ticks: *part_start_dts,
            duration_ticks: *acc_ticks,
            samples: std::mem::take(acc),
        };
        let part_name = format!("seg_{rep}_{seg_number}.{part_idx}.m4s");
        let data =
            write_media_segment(track, (*part_idx) as u32, &part_seg, *sample_index, encryption);
        fs::write(out_dir.join(&part_name), data)
            .with_context(|| format!("writing {part_name}"))?;
        media_segments.push(out_dir.join(&part_name));
        *sample_index += part_seg.samples.len() as u64;
        parts.push(PartialSegment {
            uri: part_name,
            duration: *acc_ticks as f64 / ts,
            independent,
        });
        *part_start_dts += *acc_ticks;
        *acc_ticks = 0;
        *part_idx += 1;
        Ok(())
    };

    for sample in &seg.samples {
        if !acc.is_empty() && acc_ticks >= part_ticks {
            flush(
                &mut acc,
                &mut acc_ticks,
                &mut part_idx,
                &mut sample_index,
                &mut part_start_dts,
                &mut parts,
                media_segments,
            )?;
        }
        acc_ticks += u64::from(sample.duration);
        acc.push(sample.clone());
    }
    flush(
        &mut acc,
        &mut acc_ticks,
        &mut part_idx,
        &mut sample_index,
        &mut part_start_dts,
        &mut parts,
        media_segments,
    )?;
    Ok(parts)
}

fn iso8601_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Format as UTC YYYY-MM-DDTHH:MM:SSZ without chrono.
    const SECS_PER_DAY: u64 = 86_400;
    let days = secs / SECS_PER_DAY;
    let day_secs = secs % SECS_PER_DAY;
    let h = day_secs / 3600;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant civil_from_days (UTC days since 1970-01-01 → Y-M-D).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Best-effort: append `offset_secs` to an ISO-8601 `…Z` timestamp.
fn offset_iso8601(base: &str, offset_secs: f64) -> String {
    // Parse trailing `YYYY-MM-DDTHH:MM:SSZ` only; fall back to base on failure.
    let b = base.trim().trim_end_matches('Z');
    let Some((date, time)) = b.split_once('T') else {
        return base.to_string();
    };
    let parts: Vec<_> = date.split('-').collect();
    let tparts: Vec<_> = time.split(':').collect();
    if parts.len() != 3 || tparts.len() < 3 {
        return base.to_string();
    }
    let y: i32 = parts[0].parse().unwrap_or(1970);
    let mo: u32 = parts[1].parse().unwrap_or(1);
    let d: u32 = parts[2].parse().unwrap_or(1);
    let h: u64 = tparts[0].parse().unwrap_or(0);
    let mi: u64 = tparts[1].parse().unwrap_or(0);
    let s: f64 = tparts[2].parse().unwrap_or(0.0);
    let total = days_from_civil(y, mo, d) * 86_400
        + h as i64 * 3600
        + mi as i64 * 60
        + s as i64
        + offset_secs.round() as i64;
    let secs = total.max(0) as u64;
    let days = secs / 86_400;
    let day_secs = secs % 86_400;
    let (y, mo, d) = civil_from_days(days as i64);
    let h = day_secs / 3600;
    let m = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m as u64 - 3 } else { m as u64 + 9 };
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
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

struct LoadedInput {
    format: &'static str,
    tracks: Vec<LoadedTrack>,
}

struct LoadedTrack {
    track: Track,
    samples: Vec<Sample>,
}

fn is_transport_stream(data: &[u8]) -> bool {
    if data.len() < PACKET_SIZE * 3 {
        return false;
    }
    (0..3).all(|i| data[i * PACKET_SIZE] == 0x47)
}

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

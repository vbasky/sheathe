//! `sheathe` command-line media packager (library entry point).
//!
//! A pure-Rust alternative to Shaka Packager's `packager` binary. [`run`] parses
//! args and dispatches: `probe` lists an MP4's streams; `package` demuxes,
//! fragments, and writes CMAF init + media segments plus DASH and HLS manifests.
//! Both the `sheathe-cli` and `sheathe` binaries are thin wrappers over [`run`].

mod banner;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sheathe_core::{MediaKind, Scaled, StreamInfo};
use sheathe_crypto::{ContentKey, Scheme};
use sheathe_dash::{Manifest, Representation};
use sheathe_hls::{master_playlist, media_playlist, SegmentRef, Variant};
use sheathe_mp4::{
    write_init_segment, write_media_segment, Encryption, Fragmenter, Mp4Demuxer, SegmentPolicy,
};
use std::fs;
use std::path::Path;

/// Pure-Rust HLS/DASH/CMAF media packager.
#[derive(Debug, Parser)]
#[command(name = "sheathe", version, about, long_about = None)]
struct Cli {
    /// Suppress the startup banner.
    #[arg(long, global = true)]
    no_banner: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Package one or more inputs into CMAF segments + DASH and/or HLS
    /// manifests. Multiple inputs form an ABR ladder (one rendition each).
    Package {
        /// Input media file(s). Each becomes its own rendition(s).
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<String>,
        /// Output directory.
        #[arg(short, long, default_value = "out")]
        out: String,
        /// Target segment duration in seconds.
        #[arg(long, default_value_t = 6.0)]
        segment_duration: f64,
        /// Emit a DASH manifest (`manifest.mpd`).
        #[arg(long)]
        dash: bool,
        /// Emit HLS playlists (`master.m3u8`).
        #[arg(long)]
        hls: bool,
        /// Encrypt using a raw key, as `<KID hex>:<KEY hex>` (both 16 bytes /
        /// 32 hex chars).
        #[arg(long, value_name = "KID:KEY")]
        enc_key: Option<String>,
        /// Encryption scheme when `--enc-key` is set: `cenc` (AES-CTR) or
        /// `cbcs` (AES-CBC pattern).
        #[arg(long, default_value = "cenc")]
        enc_scheme: String,
    },
    /// Probe an input and print the streams sheathe detects.
    Probe {
        /// Input media file.
        input: String,
    },
}

/// Parse CLI args and run the requested command.
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    if !cli.no_banner {
        banner::print();
    }

    match cli.command {
        Command::Package {
            inputs,
            out,
            segment_duration,
            dash,
            hls,
            enc_key,
            enc_scheme,
        } => cmd_package(
            &inputs,
            &out,
            segment_duration,
            dash,
            hls,
            enc_key.as_deref(),
            &enc_scheme,
        )?,
        Command::Probe { input } => cmd_probe(&input)?,
    }

    Ok(())
}

/// Read an MP4 and print the streams sheathe detects.
fn cmd_probe(input: &str) -> Result<()> {
    let bytes = fs::read(input).with_context(|| format!("reading {input}"))?;
    let demux = Mp4Demuxer::parse(&bytes).with_context(|| format!("parsing {input}"))?;

    println!(
        "probe: {input}  ({} bytes, {} track(s))",
        bytes.len(),
        demux.tracks().len()
    );
    for (i, track) in demux.tracks().iter().enumerate() {
        println!(
            "  [{}] track #{}  {}",
            i,
            track.track_id,
            describe(&track.info)
        );
        println!(
            "       samples={}  timescale={}",
            track.sample_count, track.info.timescale.0
        );
    }
    Ok(())
}

/// Demux, fragment, and write CMAF init + media segments plus DASH/HLS manifests
/// for one or more inputs. Each input's track(s) become separate renditions
/// sharing one manifest (an ABR ladder when several video inputs are given).
fn cmd_package(
    inputs: &[String],
    out: &str,
    segment_duration: f64,
    dash: bool,
    hls: bool,
    enc_key: Option<&str>,
    enc_scheme: &str,
) -> Result<()> {
    let out_dir = Path::new(out);
    fs::create_dir_all(out_dir).with_context(|| format!("creating {out}/"))?;
    let encryption = enc_key.map(|k| parse_enc_key(k, enc_scheme)).transpose()?;

    // Read then parse all inputs (each demuxer borrows its byte buffer).
    let datas: Vec<Vec<u8>> = inputs
        .iter()
        .map(|p| fs::read(p).with_context(|| format!("reading {p}")))
        .collect::<Result<_>>()?;
    let demuxers: Vec<Mp4Demuxer> = datas
        .iter()
        .zip(inputs)
        .map(|(d, p)| Mp4Demuxer::parse(d).with_context(|| format!("parsing {p}")))
        .collect::<Result<_>>()?;

    println!("package: {} input(s) -> {out}/", inputs.len());
    println!("  segment_duration = {segment_duration}s  (dash={dash}, hls={hls})");
    if encryption.is_some() {
        let alg = match enc_scheme {
            "cbcs" => "cbcs (AES-128-CBC pattern)",
            _ => "cenc (AES-128-CTR)",
        };
        println!("  encryption = {alg}");
    }

    let policy = SegmentPolicy {
        target_seconds: segment_duration,
        keyframes_only: true,
    };
    let mut dash_reps = Vec::new();
    let mut hls_variants = Vec::new();
    let mut total_seconds = 0.0_f64;
    let mut rep = 0usize; // global rendition index across all inputs/tracks

    for demux in &demuxers {
        for ti in 0..demux.tracks().len() {
            let track = &demux.tracks()[ti];
            let samples = demux.samples(ti)?;
            let mut frag = Fragmenter::new(track.info.clone(), policy);
            for s in samples {
                frag.push(s)?;
            }
            let segments = frag.finish();
            let ts = track.info.timescale;

            // Init segment.
            let init_name = format!("init_{rep}.mp4");
            fs::write(
                out_dir.join(&init_name),
                write_init_segment(track, encryption.as_ref()),
            )
            .with_context(|| format!("writing {init_name}"))?;

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
            println!(
                "  [{}] {}  ->  {} + {} segment(s), {:.2}s",
                rep,
                describe(&track.info),
                init_name,
                segments.len(),
                track_seconds,
            );

            dash_reps.push(Representation {
                id: rep.to_string(),
                stream: track.info.clone(),
                init: init_name.clone(),
                media: format!("seg_{rep}_$Number$.m4s"),
                timescale: ts.0,
                segment_durations: durations,
            });

            if hls {
                let media_name = format!("media_{rep}.m3u8");
                fs::write(
                    out_dir.join(&media_name),
                    media_playlist(&init_name, &hls_segs),
                )
                .with_context(|| format!("writing {media_name}"))?;
                hls_variants.push(Variant {
                    stream: track.info.clone(),
                    playlist_uri: media_name,
                });
            }

            rep += 1;
        }
    }

    if dash {
        let mpd = Manifest {
            duration_seconds: total_seconds,
            representations: dash_reps,
        }
        .to_xml();
        fs::write(out_dir.join("manifest.mpd"), mpd).context("writing manifest.mpd")?;
        println!("  wrote manifest.mpd");
    }
    if hls {
        fs::write(out_dir.join("master.m3u8"), master_playlist(&hls_variants))
            .context("writing master.m3u8")?;
        println!("  wrote master.m3u8 (+ per-track media playlists)");
    }

    Ok(())
}

/// Parse a `<KID hex>:<KEY hex>` raw-key spec + scheme name into an [`Encryption`].
fn parse_enc_key(spec: &str, scheme: &str) -> Result<Encryption> {
    let (kid_hex, key_hex) = spec
        .split_once(':')
        .context("--enc-key must be <KID hex>:<KEY hex>")?;
    let kid = parse_hex16(kid_hex).context("invalid KID")?;
    let key = parse_hex16(key_hex).context("invalid KEY")?;
    let scheme = match scheme {
        "cenc" => Scheme::Cenc,
        "cbcs" => Scheme::Cbcs,
        other => anyhow::bail!("unknown --enc-scheme '{other}' (expected cenc or cbcs)"),
    };
    // A fixed, asset-wide constant IV for cbcs (cenc derives per-sample IVs and
    // ignores this).
    let constant_iv = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    Ok(Encryption {
        scheme,
        key: ContentKey { kid, key },
        constant_iv,
    })
}

/// Parse exactly 32 hex chars into a 16-byte array.
fn parse_hex16(s: &str) -> Result<[u8; 16]> {
    let s = s.trim();
    anyhow::ensure!(s.len() == 32, "expected 32 hex chars, got {}", s.len());
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("non-hex digit")?;
    }
    Ok(out)
}

/// One-line human description of a stream.
fn describe(info: &StreamInfo) -> String {
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

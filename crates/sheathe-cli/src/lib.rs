//! `sheathe` command-line media packager (library entry point).
//!
//! A pure-Rust alternative to Shaka Packager's `packager` binary. [`run`] parses
//! args and dispatches:
//!
//! - **`package`** — demux → fragment → CMAF/TS/packed-audio segments + DASH/HLS
//! - **`probe`** — list streams sheathe detects (no packaging)
//! - **`origin`** — JIT HTTP origin (`GET /package?input=…`)
//!
//! The packaging pipeline itself lives in the reusable [`sheathe_package`] crate;
//! this module is a thin CLI over it. Both the `sheathe-cli` and `sheathe` binaries
//! are thin wrappers over [`run`].
//!
//! Full end-user command documentation: the repository's `docs/CLI.md`.

mod banner;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use sheathe_package::{
    DrmSystem, EncScheme, EncryptionSpec, OriginConfig, PackageOptions, PresentationMode,
    Scte35Marker, SegmentFormat,
};
use std::path::{Path, PathBuf};

/// Pure-Rust HLS/DASH/CMAF media packager.
#[derive(Debug, Parser)]
#[command(
    name = "sheathe",
    version,
    about = "Pure-Rust HLS/DASH/CMAF media packager",
    long_about = None
)]
struct Cli {
    /// Suppress the startup banner.
    #[arg(long, global = true)]
    no_banner: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliPresentation {
    Vod,
    Event,
    Live,
}

impl From<CliPresentation> for PresentationMode {
    fn from(p: CliPresentation) -> Self {
        match p {
            CliPresentation::Vod => PresentationMode::Vod,
            CliPresentation::Event => PresentationMode::Event,
            CliPresentation::Live => PresentationMode::Live,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliSegmentFormat {
    Cmaf,
    Ts,
    PackedAudio,
}

impl From<CliSegmentFormat> for SegmentFormat {
    fn from(f: CliSegmentFormat) -> Self {
        match f {
            CliSegmentFormat::Cmaf => SegmentFormat::Cmaf,
            CliSegmentFormat::Ts => SegmentFormat::MpegTs,
            CliSegmentFormat::PackedAudio => SegmentFormat::PackedAudio,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Package one or more inputs into CMAF segments + DASH and/or HLS
    /// manifests. Multiple inputs form an ABR ladder (one rendition each),
    /// unless `--multi-period` is set.
    Package(Box<PackageArgs>),
    /// Probe an input and print the streams sheathe detects.
    Probe {
        /// Input media file.
        input: String,
    },
    /// Run a JIT HTTP origin that packages on request.
    Origin {
        /// Bind address (default `127.0.0.1:8787`).
        #[arg(long, default_value = "127.0.0.1:8787")]
        bind: String,
        /// Sandbox root for `input=` paths.
        #[arg(long, default_value = ".")]
        media_root: String,
        /// Cache directory for packaged outputs.
        #[arg(long, default_value = "/tmp/sheathe-origin")]
        cache_dir: String,
        /// Default segment duration for JIT packages.
        #[arg(long, default_value_t = 6.0)]
        segment_duration: f64,
    },
}

/// Arguments for `sheathe package` (boxed in [`Command`] to keep the enum small).
#[derive(Debug, Parser)]
struct PackageArgs {
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
    /// Presentation mode: `vod` (default), `event`, or `live`.
    #[arg(long, value_enum, default_value_t = CliPresentation::Vod)]
    presentation: CliPresentation,
    /// Live/event sliding-window size in segments (default: 3 for live).
    #[arg(long, value_name = "N")]
    live_window: Option<usize>,
    /// Treat each input as a successive DASH Period instead of an ABR ladder.
    #[arg(long)]
    multi_period: bool,
    /// Emit trick-play (I-frame) tracks / playlists for video.
    #[arg(long)]
    trick_play: bool,
    /// Low-latency packaging (LL-HLS parts + LL-DASH availabilityTimeOffset).
    #[arg(long)]
    low_latency: bool,
    /// Part target duration in seconds when `--low-latency` is set (default 1).
    #[arg(long, value_name = "SECONDS")]
    part_duration: Option<f64>,
    /// Wall-clock availability start (ISO-8601, e.g. `2026-07-18T00:00:00Z`).
    /// Defaults to now for live/event.
    #[arg(long, value_name = "ISO8601")]
    availability_start_time: Option<String>,
    /// SCTE-35 marker as `TIME[:out|in][:BREAK_DUR]`. TIME is seconds from
    /// period start. Repeatable. Example: `--scte35 30:out:15 --scte35 45:in`.
    #[arg(long = "scte35", value_name = "SPEC", action = clap::ArgAction::Append)]
    scte35: Vec<String>,
    /// Encrypt using a raw key, as `<KID hex>:<KEY hex>` (both 16 bytes /
    /// 32 hex chars).
    #[arg(long, value_name = "KID:KEY")]
    enc_key: Option<String>,
    /// Read the raw key from a file (a `<KID hex>:<KEY hex>` line; `#`
    /// comments and blank lines ignored). Takes precedence over `--enc-key`
    /// and keeps the key out of the process arguments.
    #[arg(long, value_name = "PATH")]
    enc_key_file: Option<String>,
    /// Encryption scheme when `--enc-key` is set: `cenc` (AES-CTR),
    /// `cens` (AES-CTR pattern), `cbc1` (AES-CBC) or `cbcs` (AES-CBC pattern).
    #[arg(long, default_value = "cenc")]
    enc_scheme: String,
    /// Key-delivery URI written into the HLS `#EXT-X-KEY` tag when encrypting.
    #[arg(long, default_value = "key.bin")]
    enc_key_uri: String,
    /// DRM systems to emit `pssh` boxes for (comma-separated): any of
    /// `common`, `widevine`, `playready`.
    #[arg(long, default_value = "common")]
    protection_systems: String,
    /// Enable key rotation with this crypto-period duration in seconds. Each
    /// period uses a key derived from the base key; signalled per segment via
    /// `seig` sample groups and per-period `pssh`.
    #[arg(long, value_name = "SECONDS")]
    crypto_period_duration: Option<f64>,
    /// Segment container: `cmaf` (default), `ts` (MPEG-TS), or `packed-audio`.
    #[arg(long, value_enum, default_value_t = CliSegmentFormat::Cmaf)]
    format: CliSegmentFormat,
    /// DASH on-demand single-file output (`SegmentList` byte ranges).
    #[arg(long)]
    on_demand: bool,
    /// Parallelise per-track packaging across threads.
    #[arg(long)]
    parallel: bool,
    /// After writing locally, HTTP PUT every object to this base URL.
    #[arg(long, value_name = "URL")]
    http_push: Option<String>,
}

/// Parse CLI args and run the requested command.
pub fn run() -> Result<()> {
    // Clap handles `--help` / `--version` inside `parse()` and exits before
    // returning, so the banner must be printed first.
    if !std::env::args().any(|a| a == "--no-banner") {
        banner::print();
    }

    let cli = Cli::parse();

    match cli.command {
        Command::Package(p) => cmd_package(PackageCli {
            inputs: &p.inputs,
            out: &p.out,
            segment_duration: p.segment_duration,
            dash: p.dash,
            hls: p.hls,
            presentation: p.presentation.into(),
            live_window: p.live_window,
            multi_period: p.multi_period,
            trick_play: p.trick_play,
            low_latency: p.low_latency,
            part_duration: p.part_duration,
            availability_start_time: p.availability_start_time.as_deref(),
            scte35: &p.scte35,
            segment_format: p.format.into(),
            on_demand: p.on_demand,
            parallel: p.parallel,
            http_push: p.http_push.as_deref(),
            enc: EncryptionArgs {
                key: p.enc_key.as_deref(),
                key_file: p.enc_key_file.as_deref(),
                scheme: &p.enc_scheme,
                key_uri: &p.enc_key_uri,
                systems: &p.protection_systems,
                crypto_period: p.crypto_period_duration,
            },
        })?,
        Command::Probe { input } => cmd_probe(&input)?,
        Command::Origin { bind, media_root, cache_dir, segment_duration } => {
            sheathe_package::serve_origin(OriginConfig {
                bind,
                media_root: PathBuf::from(media_root),
                cache_dir: PathBuf::from(cache_dir),
                segment_duration,
            })?;
        }
    }

    Ok(())
}

/// Read an input file and print the streams sheathe detects.
fn cmd_probe(input: &str) -> Result<()> {
    let report = sheathe_package::probe(Path::new(input))?;
    println!(
        "probe: {input}  ({} bytes, {} track(s), {})",
        report.size_bytes,
        report.streams.len(),
        report.format
    );
    for (i, s) in report.streams.iter().enumerate() {
        println!("  [{}] track #{}  {}", i, s.track_id, sheathe_package::describe(&s.info));
        println!("       samples={}  timescale={}", s.sample_count, s.info.timescale.0);
    }
    Ok(())
}

struct EncryptionArgs<'a> {
    key: Option<&'a str>,
    key_file: Option<&'a str>,
    scheme: &'a str,
    key_uri: &'a str,
    systems: &'a str,
    crypto_period: Option<f64>,
}

struct PackageCli<'a> {
    inputs: &'a [String],
    out: &'a str,
    segment_duration: f64,
    dash: bool,
    hls: bool,
    presentation: PresentationMode,
    live_window: Option<usize>,
    multi_period: bool,
    trick_play: bool,
    low_latency: bool,
    part_duration: Option<f64>,
    availability_start_time: Option<&'a str>,
    scte35: &'a [String],
    segment_format: SegmentFormat,
    on_demand: bool,
    parallel: bool,
    http_push: Option<&'a str>,
    enc: EncryptionArgs<'a>,
}

fn cmd_package(args: PackageCli<'_>) -> Result<()> {
    let key_spec = match args.enc.key_file {
        Some(path) => Some(read_key_file(path)?),
        None => args.enc.key.map(str::to_string),
    };
    let encryption = key_spec
        .map(|k| {
            parse_enc_key(
                &k,
                args.enc.scheme,
                args.enc.key_uri,
                args.enc.systems,
                args.enc.crypto_period,
            )
        })
        .transpose()?;

    let markers = args.scte35.iter().map(|s| parse_scte35(s)).collect::<Result<Vec<_>>>()?;

    let mode = match args.presentation {
        PresentationMode::Vod => "vod",
        PresentationMode::Event => "event",
        PresentationMode::Live => "live",
    };
    println!("package: {} input(s) -> {}/", args.inputs.len(), args.out);
    println!(
        "  segment_duration = {}s  (dash={}, hls={}, presentation={mode})",
        args.segment_duration, args.dash, args.hls
    );
    if args.multi_period {
        println!("  multi_period = true");
    }
    if args.trick_play {
        println!("  trick_play = true");
    }
    if args.low_latency {
        println!("  low_latency = true  (part_duration = {}s)", args.part_duration.unwrap_or(1.0));
    }
    if let Some(w) = args.live_window {
        println!("  live_window = {w} segment(s)");
    }
    if !markers.is_empty() {
        println!("  scte35 markers = {}", markers.len());
    }
    if encryption.is_some() {
        let alg = match args.enc.scheme {
            "cens" => "cens (AES-128-CTR pattern)",
            "cbc1" => "cbc1 (AES-128-CBC)",
            "cbcs" => "cbcs (AES-128-CBC pattern)",
            _ => "cenc (AES-128-CTR)",
        };
        println!("  encryption = {alg}");
        println!("  protection_systems = {}", args.enc.systems);
        if let Some(p) = args.enc.crypto_period {
            println!("  key_rotation = every {p}s (crypto period)");
        }
    }

    let input_paths: Vec<PathBuf> = args.inputs.iter().map(PathBuf::from).collect();
    if args.on_demand {
        println!("  on_demand = true (DASH SegmentList single-file)");
    }
    if args.parallel {
        println!("  parallel = true");
    }
    if let Some(u) = args.http_push {
        println!("  http_push = {u}");
    }
    match args.segment_format {
        SegmentFormat::Cmaf => {}
        SegmentFormat::MpegTs => println!("  format = mpeg-ts"),
        SegmentFormat::PackedAudio => println!("  format = packed-audio"),
    }

    let opts = PackageOptions {
        out_dir: PathBuf::from(args.out),
        segment_duration: args.segment_duration,
        dash: args.dash,
        hls: args.hls,
        encryption,
        presentation: args.presentation,
        live_window_segments: args.live_window,
        multi_period: args.multi_period,
        trick_play: args.trick_play,
        low_latency: args.low_latency,
        part_duration: args.part_duration,
        availability_start_time: args.availability_start_time.map(str::to_string),
        scte35_markers: markers,
        segment_format: args.segment_format,
        on_demand: args.on_demand,
        parallel: args.parallel,
        http_push_url: args.http_push.map(str::to_string),
    };
    let output = sheathe_package::package(&input_paths, &opts)?;

    println!(
        "  {} rendition(s), {:.2}s, {} segment(s)",
        output.renditions,
        output.duration_seconds,
        output.media_segments.len(),
    );
    if let Some(p) = &output.dash_manifest {
        println!("  wrote {}", p.display());
    }
    if let Some(p) = &output.hls_master {
        println!("  wrote {} (+ per-track media playlists)", p.display());
    }

    Ok(())
}

/// Parse `--scte35 TIME[:out|in][:BREAK_DUR]`.
fn parse_scte35(spec: &str) -> Result<Scte35Marker> {
    let parts: Vec<&str> = spec.split(':').collect();
    anyhow::ensure!(!parts.is_empty(), "empty --scte35 spec");
    let time: f64 = parts[0].parse().context("--scte35 TIME must be a number of seconds")?;
    anyhow::ensure!(time >= 0.0, "--scte35 TIME must be non-negative");
    let mut out_of_network = true;
    let mut break_duration = None;
    if parts.len() >= 2 {
        match parts[1].to_ascii_lowercase().as_str() {
            "out" | "cue-out" | "cue_out" => out_of_network = true,
            "in" | "cue-in" | "cue_in" => out_of_network = false,
            other => {
                // Bare TIME:DURATION form.
                break_duration = Some(other.parse::<f64>().with_context(|| {
                    format!("--scte35: expected out/in or duration, got '{other}'")
                })?);
            }
        }
    }
    if parts.len() >= 3 {
        break_duration =
            Some(parts[2].parse::<f64>().context("--scte35 BREAK_DUR must be a number")?);
    }
    Ok(Scte35Marker {
        time_seconds: time,
        out_of_network,
        event_id: None,
        break_duration_seconds: break_duration,
    })
}

fn parse_enc_key(
    spec: &str,
    scheme: &str,
    key_uri: &str,
    systems: &str,
    crypto_period: Option<f64>,
) -> Result<EncryptionSpec> {
    let (kid_hex, key_hex) =
        spec.split_once(':').context("--enc-key must be <KID hex>:<KEY hex>")?;
    let kid = parse_hex16(kid_hex).context("invalid KID")?;
    let key = parse_hex16(key_hex).context("invalid KEY")?;
    let scheme = match scheme {
        "cenc" => EncScheme::Cenc,
        "cens" => EncScheme::Cens,
        "cbc1" => EncScheme::Cbc1,
        "cbcs" => EncScheme::Cbcs,
        other => {
            anyhow::bail!("unknown --enc-scheme '{other}' (expected cenc, cens, cbc1 or cbcs)")
        }
    };
    let systems = parse_protection_systems(systems)?;
    if let Some(p) = crypto_period {
        anyhow::ensure!(p > 0.0, "--crypto-period-duration must be positive");
    }
    Ok(EncryptionSpec {
        kid,
        key,
        scheme,
        key_uri: key_uri.to_string(),
        systems,
        crypto_period_seconds: crypto_period,
    })
}

fn read_key_file(path: &str) -> Result<String> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading key file {path}"))?;
    content
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .find(|line| line.contains(':'))
        .map(str::to_string)
        .with_context(|| format!("no <KID hex>:<KEY hex> entry in key file {path}"))
}

fn parse_protection_systems(list: &str) -> Result<Vec<DrmSystem>> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|name| {
            DrmSystem::parse(name).with_context(|| {
                format!("unknown protection system '{name}' (expected common, widevine, playready)")
            })
        })
        .collect()
}

fn parse_hex16(s: &str) -> Result<[u8; 16]> {
    let s = s.trim();
    anyhow::ensure!(s.len() == 32, "expected 32 hex chars, got {}", s.len());
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).context("non-hex digit")?;
    }
    Ok(out)
}

//! `sheathe` command-line media packager (library entry point).
//!
//! A pure-Rust alternative to Shaka Packager's `packager` binary. [`run`] parses
//! args and dispatches: `probe` lists an input's streams; `package` demuxes,
//! fragments, and writes CMAF init + media segments plus DASH and HLS manifests.
//! The packaging pipeline itself lives in the reusable [`sheathe_package`] crate;
//! this module is a thin CLI over it. Both the `sheathe-cli` and `sheathe` binaries
//! are thin wrappers over [`run`].

mod banner;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sheathe_package::{DrmSystem, EncScheme, EncryptionSpec, PackageOptions};
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
    },
    /// Probe an input and print the streams sheathe detects.
    Probe {
        /// Input media file.
        input: String,
    },
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
        Command::Package {
            inputs,
            out,
            segment_duration,
            dash,
            hls,
            enc_key,
            enc_key_file,
            enc_scheme,
            enc_key_uri,
            protection_systems,
            crypto_period_duration,
        } => cmd_package(
            &inputs,
            &out,
            segment_duration,
            dash,
            hls,
            EncryptionArgs {
                key: enc_key.as_deref(),
                key_file: enc_key_file.as_deref(),
                scheme: &enc_scheme,
                key_uri: &enc_key_uri,
                systems: &protection_systems,
                crypto_period: crypto_period_duration,
            },
        )?,
        Command::Probe { input } => cmd_probe(&input)?,
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

/// Encryption-related CLI options, grouped so `cmd_package` stays tidy.
struct EncryptionArgs<'a> {
    /// `<KID hex>:<KEY hex>` raw key, or `None` for clear output.
    key: Option<&'a str>,
    /// Path to a file holding the raw key; takes precedence over `key`.
    key_file: Option<&'a str>,
    /// `cenc`, `cens`, `cbc1` or `cbcs`.
    scheme: &'a str,
    /// HLS `#EXT-X-KEY` delivery URI.
    key_uri: &'a str,
    /// Comma-separated DRM systems to emit `pssh` boxes for.
    systems: &'a str,
    /// Key-rotation crypto-period duration in seconds, or `None` for one key.
    crypto_period: Option<f64>,
}

fn cmd_package(
    inputs: &[String],
    out: &str,
    segment_duration: f64,
    dash: bool,
    hls: bool,
    enc: EncryptionArgs<'_>,
) -> Result<()> {
    // The key file (if given) wins over an inline --enc-key.
    let key_spec = match enc.key_file {
        Some(path) => Some(read_key_file(path)?),
        None => enc.key.map(str::to_string),
    };
    let encryption = key_spec
        .map(|k| parse_enc_key(&k, enc.scheme, enc.key_uri, enc.systems, enc.crypto_period))
        .transpose()?;

    println!("package: {} input(s) -> {out}/", inputs.len());
    println!("  segment_duration = {segment_duration}s  (dash={dash}, hls={hls})");
    if encryption.is_some() {
        let alg = match enc.scheme {
            "cens" => "cens (AES-128-CTR pattern)",
            "cbc1" => "cbc1 (AES-128-CBC)",
            "cbcs" => "cbcs (AES-128-CBC pattern)",
            _ => "cenc (AES-128-CTR)",
        };
        println!("  encryption = {alg}");
        println!("  protection_systems = {}", enc.systems);
        if let Some(p) = enc.crypto_period {
            println!("  key_rotation = every {p}s (crypto period)");
        }
    }

    let input_paths: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
    let opts =
        PackageOptions { out_dir: PathBuf::from(out), segment_duration, dash, hls, encryption };
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

/// Parse a `<KID hex>:<KEY hex>` raw-key spec, scheme name, and DRM-system list
/// into an [`EncryptionSpec`].
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

/// Read a raw key from a file: the first `<KID hex>:<KEY hex>` line, ignoring
/// blank lines and `#` comments.
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

/// Parse a comma-separated DRM-system list (e.g. `common,widevine,playready`).
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

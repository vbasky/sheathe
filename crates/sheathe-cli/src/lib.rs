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
//! this module is a thin CLI over it. `cargo install sheathe-cli` installs the
//! `sheathe` binary, which is a thin wrapper over [`run`].
//!
//! Full end-user command documentation: the repository's `docs/CLI.md`.

mod banner;
mod help;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use sheathe_package::{
    CpixConfig, DrmSystem, EncScheme, EncryptionSpec, OriginConfig, PackageKnobs, PackageOptions,
    PresentationMode, Scte35Marker, SegmentFormat, WidevineConfig, parse_input_arg,
};
use std::path::{Path, PathBuf};

/// Pure-Rust HLS/DASH/CMAF media packager.
#[derive(Debug, Parser)]
#[command(
    name = "sheathe",
    version,
    about = "Pure-Rust HLS/DASH/CMAF media packager",
    long_about = help::ROOT_LONG_ABOUT,
    after_long_help = help::ROOT_AFTER_HELP,
    arg_required_else_help = true
)]
struct Cli {
    /// Suppress the startup banner.
    #[arg(long, global = true, long_help = help::NO_BANNER)]
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
    #[command(
        long_about = help::PACKAGE_LONG_ABOUT,
        after_long_help = help::PACKAGE_AFTER_HELP
    )]
    Package(Box<PackageArgs>),
    /// Probe an input and print the streams sheathe detects.
    #[command(long_about = help::PROBE_LONG_ABOUT, after_long_help = help::PROBE_AFTER_HELP)]
    Probe {
        /// Input media file (same formats as `package`). No packaging is performed.
        input: String,
    },
    /// Run a JIT HTTP origin that packages on request.
    #[command(long_about = help::ORIGIN_LONG_ABOUT, after_long_help = help::ORIGIN_AFTER_HELP)]
    Origin {
        /// Bind address (default `127.0.0.1:8787`).
        #[arg(long, default_value = "127.0.0.1:8787", long_help = help::ORIGIN_BIND)]
        bind: String,
        /// Sandbox root for `input=` paths.
        #[arg(long, default_value = ".", long_help = help::ORIGIN_MEDIA_ROOT)]
        media_root: String,
        /// Cache directory for packaged outputs.
        #[arg(long, default_value = "/tmp/sheathe-origin", long_help = help::ORIGIN_CACHE_DIR)]
        cache_dir: String,
        /// Default segment duration for JIT packages.
        #[arg(long, default_value_t = 6.0, long_help = help::ORIGIN_SEGMENT_DURATION)]
        segment_duration: f64,
        /// TLS certificate PEM (enables HTTPS).
        #[arg(long, help_heading = "Live / IO")]
        tls_cert: Option<String>,
        /// TLS private key PEM.
        #[arg(long, help_heading = "Live / IO")]
        tls_key: Option<String>,
        /// HTTP Basic auth as `user:password`.
        #[arg(long, help_heading = "Live / IO")]
        auth: Option<String>,
    },
}

/// Arguments for `sheathe package` (boxed in [`Command`] to keep the enum small).
#[derive(Debug, Parser)]
struct PackageArgs {
    /// Input media file(s). Each becomes its own rendition(s).
    #[arg(required = true, num_args = 1.., long_help = help::INPUTS, help_heading = "Arguments")]
    inputs: Vec<String>,
    /// Output directory.
    #[arg(short, long, default_value = "out", long_help = help::OUT, help_heading = "Output")]
    out: String,
    /// Target segment duration in seconds.
    #[arg(
        long,
        default_value_t = 6.0,
        long_help = help::SEGMENT_DURATION,
        help_heading = "Output"
    )]
    segment_duration: f64,
    /// Emit a DASH manifest (`manifest.mpd`).
    #[arg(long, long_help = help::DASH, help_heading = "Output")]
    dash: bool,
    /// Emit HLS playlists (`master.m3u8`).
    #[arg(long, long_help = help::HLS, help_heading = "Output")]
    hls: bool,
    /// Segment container: `cmaf` (default), `ts` (MPEG-TS), or `packed-audio`.
    #[arg(
        long,
        value_enum,
        default_value_t = CliSegmentFormat::Cmaf,
        long_help = help::FORMAT,
        help_heading = "Output"
    )]
    format: CliSegmentFormat,
    /// DASH on-demand single-file output (`SegmentList` byte ranges).
    #[arg(long, long_help = help::ON_DEMAND, help_heading = "Output")]
    on_demand: bool,
    /// Parallelise per-track packaging across threads.
    #[arg(long, long_help = help::PARALLEL, help_heading = "Output")]
    parallel: bool,
    /// After writing locally, HTTP PUT every object to this base URL.
    #[arg(long, value_name = "URL", long_help = help::HTTP_PUSH, help_heading = "Output")]
    http_push: Option<String>,
    /// Presentation mode: `vod` (default), `event`, or `live`.
    #[arg(
        long,
        value_enum,
        default_value_t = CliPresentation::Vod,
        long_help = help::PRESENTATION,
        help_heading = "Presentation"
    )]
    presentation: CliPresentation,
    /// Live/event sliding-window size in segments (default: 3 for live).
    #[arg(long, value_name = "N", long_help = help::LIVE_WINDOW, help_heading = "Presentation")]
    live_window: Option<usize>,
    /// Treat each input as a successive DASH Period instead of an ABR ladder.
    #[arg(long, long_help = help::MULTI_PERIOD, help_heading = "Presentation")]
    multi_period: bool,
    /// Emit trick-play (I-frame) tracks / playlists for video.
    #[arg(long, long_help = help::TRICK_PLAY, help_heading = "Presentation")]
    trick_play: bool,
    /// Low-latency packaging (LL-HLS parts + LL-DASH availabilityTimeOffset).
    #[arg(long, long_help = help::LOW_LATENCY, help_heading = "Presentation")]
    low_latency: bool,
    /// Part target duration in seconds when `--low-latency` is set (default 1).
    #[arg(
        long,
        value_name = "SECONDS",
        long_help = help::PART_DURATION,
        help_heading = "Presentation"
    )]
    part_duration: Option<f64>,
    /// Wall-clock availability start (ISO-8601, e.g. `2026-07-18T00:00:00Z`).
    /// Defaults to now for live/event.
    #[arg(
        long,
        value_name = "ISO8601",
        long_help = help::AVAILABILITY_START_TIME,
        help_heading = "Presentation"
    )]
    availability_start_time: Option<String>,
    /// SCTE-35 marker as `TIME[:out|in][:BREAK_DUR]`. Repeatable.
    #[arg(
        long = "scte35",
        value_name = "SPEC",
        action = clap::ArgAction::Append,
        long_help = help::SCTE35,
        help_heading = "Presentation"
    )]
    scte35: Vec<String>,
    /// Encrypt using a raw key, as `<KID hex>:<KEY hex>` (both 16 bytes /
    /// 32 hex chars).
    #[arg(long, value_name = "KID:KEY", long_help = help::ENC_KEY, help_heading = "Encryption")]
    enc_key: Option<String>,
    /// Read the raw key from a file. Takes precedence over `--enc-key`.
    #[arg(long, value_name = "PATH", long_help = help::ENC_KEY_FILE, help_heading = "Encryption")]
    enc_key_file: Option<String>,
    /// Encryption scheme when a key is set: `cenc`, `cens`, `cbc1`, or `cbcs`.
    #[arg(long, default_value = "cenc", long_help = help::ENC_SCHEME, help_heading = "Encryption")]
    enc_scheme: String,
    /// Key-delivery URI written into the HLS `#EXT-X-KEY` tag when encrypting.
    #[arg(
        long,
        default_value = "key.bin",
        long_help = help::ENC_KEY_URI,
        help_heading = "Encryption"
    )]
    enc_key_uri: String,
    /// DRM systems to emit `pssh` boxes for: `common`, `widevine`, `playready`.
    #[arg(
        long,
        default_value = "common",
        long_help = help::PROTECTION_SYSTEMS,
        help_heading = "Encryption"
    )]
    protection_systems: String,
    /// Enable key rotation with this crypto-period duration in seconds.
    #[arg(
        long,
        value_name = "SECONDS",
        long_help = help::CRYPTO_PERIOD_DURATION,
        help_heading = "Encryption"
    )]
    crypto_period_duration: Option<f64>,
    #[command(flatten)]
    extra: ExtraKnobs,
}

/// Shaka-parity knobs (HLS/DASH, DRM extras, Live/IO).
#[derive(Debug, Parser)]
struct ExtraKnobs {
    /// HLS base URL prefix for playlist URIs.
    #[arg(long, help_heading = "HLS/DASH")]
    hls_base_url: Option<String>,
    /// Initial EXT-X-MEDIA-SEQUENCE.
    #[arg(long, default_value_t = 0, help_heading = "HLS/DASH")]
    hls_media_sequence_number: u64,
    /// EXT-X-START TIME-OFFSET in seconds.
    #[arg(long, help_heading = "HLS/DASH")]
    hls_start_time_offset: Option<f64>,
    /// Emit EXT-X-SESSION-KEY in the master playlist.
    #[arg(long, help_heading = "HLS/DASH")]
    create_session_keys: bool,
    /// Add EXT-X-PROGRAM-DATE-TIME on every segment.
    #[arg(long, help_heading = "HLS/DASH")]
    add_program_date_time: bool,
    /// CEA closed captions: channel=CC1,name=English,lang=eng;…
    #[arg(long, help_heading = "HLS/DASH")]
    closed_captions: Option<String>,
    /// Use legacy `vp9` codec string instead of `vp09.…`.
    #[arg(long, help_heading = "HLS/DASH")]
    use_legacy_vp9_codec_string: bool,
    /// DASH-IF last-segment-number SupplementalProperty.
    #[arg(long, help_heading = "HLS/DASH")]
    dash_add_last_segment_number: bool,
    /// SegmentTemplate@duration when durations are uniform.
    #[arg(long, help_heading = "HLS/DASH")]
    segment_template_constant_duration: bool,
    /// Signal Dolby Vision via supplemental codecs.
    #[arg(long, help_heading = "HLS/DASH")]
    use_dovi_supplemental_codecs: bool,
    /// Write mvex before trak (Android MediaExtractor).
    #[arg(long, help_heading = "HLS/DASH")]
    mvex_before_trak: bool,
    /// Pattern crypt byte blocks (cbcs/cens). Default 1.
    #[arg(long, default_value_t = 1, help_heading = "Encryption")]
    crypt_byte_block: u8,
    /// Pattern skip byte blocks (cbcs/cens). Default 9.
    #[arg(long, default_value_t = 9, help_heading = "Encryption")]
    skip_byte_block: u8,
    /// Extra XML inside the PlayReady WRMHEADER.
    #[arg(long, help_heading = "Encryption")]
    playready_extra_header_data: Option<String>,
    /// Labeled keys: label=AUDIO:key_id=hex:key=hex,label=SD:…
    #[arg(long, help_heading = "Encryption")]
    keys: Option<String>,
    /// Decrypt CENC inputs with the raw key before re-packaging.
    #[arg(long, help_heading = "Encryption")]
    decrypt: bool,
    /// CPIX document path or URL.
    #[arg(long, help_heading = "Encryption")]
    cpix: Option<String>,
    /// SPEKE: POST this CPIX request file to --cpix URL.
    #[arg(long, help_heading = "Encryption")]
    cpix_request_file: Option<String>,
    /// RSA private key (PEM) for encrypted CPIX documents.
    #[arg(long, help_heading = "Encryption")]
    cpix_private_key: Option<String>,
    /// Widevine key-server URL.
    #[arg(long, help_heading = "Encryption")]
    key_server_url: Option<String>,
    /// Enable Widevine encryption (fetch keys from --key-server-url).
    #[arg(long, help_heading = "Encryption")]
    enable_widevine_encryption: bool,
    /// Enable Widevine decryption.
    #[arg(long, help_heading = "Encryption")]
    enable_widevine_decryption: bool,
    /// Widevine content id (hex).
    #[arg(long, help_heading = "Encryption")]
    content_id: Option<String>,
    /// Widevine signer name.
    #[arg(long, help_heading = "Encryption")]
    signer: Option<String>,
    /// AES-256 signing key (hex) for Widevine.
    #[arg(long, help_heading = "Encryption")]
    aes_signing_key: Option<String>,
    /// AES signing IV (hex) for Widevine.
    #[arg(long, help_heading = "Encryption")]
    aes_signing_iv: Option<String>,
    /// PKCS#1/PKCS#8 RSA private key path for Widevine.
    #[arg(long, help_heading = "Encryption")]
    rsa_signing_key_path: Option<String>,
    /// Widevine policy name.
    #[arg(long, help_heading = "Encryption")]
    policy: Option<String>,
    /// Ignore HTTP PUT failures (live ingest).
    #[arg(long, help_heading = "Live / IO")]
    ignore_http_output_failures: bool,
    /// Custom User-Agent for HTTP(S).
    #[arg(long, help_heading = "Live / IO")]
    user_agent: Option<String>,
    /// Extra CA file (PEM) for TLS.
    #[arg(long, help_heading = "Live / IO")]
    ca_file: Option<String>,
    /// Client certificate (PEM) for TLS.
    #[arg(long, help_heading = "Live / IO")]
    client_cert_file: Option<String>,
    /// Client certificate private key (PEM).
    #[arg(long, help_heading = "Live / IO")]
    client_cert_private_key_file: Option<String>,
    /// Skip adding extra TLS checks (roots still enforced).
    #[arg(long, help_heading = "Live / IO")]
    disable_peer_verification: bool,
    /// Rewrite live manifests in a loop until interrupted.
    #[arg(long, help_heading = "Live / IO")]
    live_rewrite: bool,
}

/// True when the user ran `sheathe` (optionally with `--no-banner`) and nothing
/// else. Clap treats a missing subcommand as an error (stderr + exit 2); we
/// print help to stdout and succeed instead, matching `sheathe --help`.
fn is_bare_invocation<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().skip(1).all(|a| a.as_ref() == "--no-banner")
}

/// Parse CLI args and run the requested command.
pub fn run() -> Result<()> {
    // Clap handles `--help` / `--version` inside `parse()` and exits before
    // returning, so the banner must be printed first.
    if !std::env::args().any(|a| a == "--no-banner") {
        banner::print();
    }

    if is_bare_invocation(std::env::args()) {
        Cli::command().print_help()?;
        println!();
        return Ok(());
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
            extra: &p.extra,
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
        Command::Origin {
            bind,
            media_root,
            cache_dir,
            segment_duration,
            tls_cert,
            tls_key,
            auth,
        } => {
            sheathe_package::serve_origin(OriginConfig {
                bind,
                media_root: PathBuf::from(media_root),
                cache_dir: PathBuf::from(cache_dir),
                segment_duration,
                tls_cert: tls_cert.map(PathBuf::from),
                tls_key: tls_key.map(PathBuf::from),
                basic_auth: auth,
            })?;
        }
    }

    Ok(())
}

/// Read an input file and print the streams sheathe detects.
fn cmd_probe(input: &str) -> Result<()> {
    let report = sheathe_package::probe(Path::new(input))?;
    println!("File \"{input}\":");
    println!(" container: {}", report.format);
    println!(" size: {} bytes", report.size_bytes);
    println!("Found {} stream(s).", report.streams.len());
    for (i, s) in report.streams.iter().enumerate() {
        println!();
        println!("{}", sheathe_package::dump_stream(i, s));
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
    extra: &'a ExtraKnobs,
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
        .transpose()?
        .map(|mut e| {
            e.hls_aes128 = args.enc.scheme == "aes128";
            e.crypt_byte_block = args.extra.crypt_byte_block;
            e.skip_byte_block = args.extra.skip_byte_block;
            e.playready_extra = args.extra.playready_extra_header_data.clone();
            e
        });

    let markers = args.scte35.iter().map(|s| parse_scte35(s)).collect::<Result<Vec<_>>>()?;

    let mode = match args.presentation {
        PresentationMode::Vod => "vod",
        PresentationMode::Event => "event",
        PresentationMode::Live => "live",
    };
    let format = match args.segment_format {
        SegmentFormat::Cmaf => "cmaf",
        SegmentFormat::MpegTs => "ts",
        SegmentFormat::PackedAudio => "packed-audio",
    };
    let manifests = match (args.dash, args.hls) {
        (true, true) => "DASH (manifest.mpd), HLS (master.m3u8)",
        (true, false) => "DASH (manifest.mpd)",
        (false, true) => "HLS (master.m3u8)",
        (false, false) => "none (segments only)",
    };

    println!("package: {} input(s) -> {}/", args.inputs.len(), args.out);
    for (i, input) in args.inputs.iter().enumerate() {
        println!("  input[{i}]: {input}");
    }
    println!("  segment_duration: {}s", args.segment_duration);
    println!("  format:           {format}");
    println!("  presentation:     {mode}");
    println!("  manifests:        {manifests}");
    if args.multi_period {
        println!("  multi_period:     true (each input is a DASH Period)");
    }
    if args.trick_play {
        println!("  trick_play:       true (I-frame tracks / playlists)");
    }
    if args.low_latency {
        println!(
            "  low_latency:      true (part_duration = {}s)",
            args.part_duration.unwrap_or(1.0)
        );
    }
    if let Some(w) = args.live_window {
        println!("  live_window:      {w} segment(s)");
    }
    if let Some(t) = args.availability_start_time {
        println!("  availability_start_time: {t}");
    }
    if !markers.is_empty() {
        println!("  scte35:           {} marker(s)", markers.len());
    }
    if encryption.is_some() {
        let alg = match args.enc.scheme {
            "cens" => "cens (AES-128-CTR pattern)",
            "cbc1" => "cbc1 (AES-128-CBC)",
            "cbcs" => "cbcs (AES-128-CBC pattern)",
            _ => "cenc (AES-128-CTR)",
        };
        println!("  encryption:       {alg}");
        println!("  protection_systems: {}", args.enc.systems);
        println!("  enc_key_uri:      {}", args.enc.key_uri);
        if let Some(p) = args.enc.crypto_period {
            println!("  key_rotation:     every {p}s (crypto period)");
        }
    }
    if args.on_demand {
        println!("  on_demand:        true (DASH SegmentList single-file)");
    }
    if args.parallel {
        println!("  parallel:         true");
    }
    if let Some(u) = args.http_push {
        println!("  http_push:        {u}");
    }

    let descriptors: Vec<_> =
        args.inputs.iter().map(|s| parse_input_arg(s)).collect::<Result<_>>()?;
    let input_paths: Vec<PathBuf> = descriptors.iter().map(|d| d.input.clone()).collect();
    let extra = args.extra;
    let mut knobs = PackageKnobs {
        hls_base_url: extra.hls_base_url.clone(),
        hls_media_sequence_number: extra.hls_media_sequence_number,
        hls_start_time_offset: extra.hls_start_time_offset,
        create_session_keys: extra.create_session_keys,
        add_program_date_time: extra.add_program_date_time,
        closed_captions: extra
            .closed_captions
            .as_deref()
            .map(sheathe_package::ClosedCaption::parse_list)
            .transpose()?
            .unwrap_or_default(),
        use_legacy_vp9_codec_string: extra.use_legacy_vp9_codec_string,
        dash_add_last_segment_number: extra.dash_add_last_segment_number,
        segment_template_constant_duration: extra.segment_template_constant_duration,
        use_dovi_supplemental_codecs: extra.use_dovi_supplemental_codecs,
        mvex_before_trak: extra.mvex_before_trak,
        crypt_byte_block: extra.crypt_byte_block,
        skip_byte_block: extra.skip_byte_block,
        playready_extra_header_data: extra.playready_extra_header_data.clone(),
        hls_aes128: args.enc.scheme == "aes128",
        labeled_keys: extra
            .keys
            .as_deref()
            .map(sheathe_package::LabeledKey::parse_list)
            .transpose()?
            .unwrap_or_default(),
        decrypt_input: extra.decrypt || extra.enable_widevine_decryption,
        ignore_http_output_failures: extra.ignore_http_output_failures,
        user_agent: extra.user_agent.clone(),
        ca_file: extra.ca_file.as_ref().map(PathBuf::from),
        client_cert_file: extra.client_cert_file.as_ref().map(PathBuf::from),
        client_cert_key_file: extra.client_cert_private_key_file.as_ref().map(PathBuf::from),
        disable_peer_verification: extra.disable_peer_verification,
        live_rewrite: extra.live_rewrite,
        descriptors,
        ..PackageKnobs::default()
    };
    if extra.enable_widevine_encryption || extra.enable_widevine_decryption {
        let url = extra.key_server_url.clone().context("--key-server-url required")?;
        knobs.widevine = Some(WidevineConfig {
            key_server_url: url,
            content_id: extra
                .content_id
                .as_deref()
                .map(parse_hex_bytes)
                .transpose()?
                .unwrap_or_default(),
            signer: extra.signer.clone().unwrap_or_default(),
            aes_signing_key: extra.aes_signing_key.as_deref().map(parse_hex_bytes).transpose()?,
            aes_signing_iv: extra.aes_signing_iv.as_deref().map(parse_hex_bytes).transpose()?,
            rsa_signing_key_pem: extra
                .rsa_signing_key_path
                .as_ref()
                .map(std::fs::read_to_string)
                .transpose()?,
            policy: extra.policy.clone().unwrap_or_default(),
            group_id: None,
            enable_entitlement_license: false,
            decrypt: extra.enable_widevine_decryption,
        });
    }
    if let Some(cpix) = &extra.cpix {
        knobs.cpix = Some(CpixConfig {
            path_or_url: cpix.clone(),
            headers: Vec::new(),
            private_key_pem: extra
                .cpix_private_key
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok()),
            request_file: extra.cpix_request_file.as_ref().map(PathBuf::from),
            encrypt: true,
            decrypt: extra.decrypt,
        });
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
        knobs,
    };
    let output = if extra.live_rewrite {
        loop {
            let out = sheathe_package::package(&input_paths, &opts)?;
            eprintln!(
                "live-rewrite: {} rendition(s), sleeping {}s (Ctrl-C to stop)",
                out.renditions, opts.segment_duration
            );
            std::thread::sleep(std::time::Duration::from_secs_f64(opts.segment_duration.max(0.2)));
        }
    } else {
        sheathe_package::package(&input_paths, &opts)?
    };

    println!();
    println!(
        "  {} rendition(s), {:.3}s, {} media segment(s)",
        output.renditions,
        output.duration_seconds,
        output.media_segments.len(),
    );
    if !output.init_segments.is_empty() {
        println!("  init:");
        for p in &output.init_segments {
            println!("    {}", p.display());
        }
    }
    if let Some(p) = &output.dash_manifest {
        println!("  dash:             {}", p.display());
    }
    if let Some(p) = &output.hls_master {
        println!("  hls:              {} (+ per-track media playlists)", p.display());
    }
    println!("Packaging completed successfully.");

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
        "aes128" => EncScheme::Cenc, // actual AES-128 is flagged via hls_aes128
        other => {
            anyhow::bail!(
                "unknown --enc-scheme '{other}' (expected cenc, cens, cbc1, cbcs, aes128)"
            )
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
        hls_aes128: false,
        crypt_byte_block: 1,
        skip_byte_block: 9,
        playready_extra: None,
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

fn parse_hex_bytes(s: &str) -> Result<Vec<u8>> {
    let s = s.trim().trim_start_matches("0x");
    anyhow::ensure!(s.len() % 2 == 0, "hex string must have even length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).context("non-hex digit"))
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

#[cfg(test)]
mod tests {
    use super::{Cli, is_bare_invocation};
    use clap::CommandFactory;

    #[test]
    fn bare_invocation_is_just_the_binary() {
        assert!(is_bare_invocation(["sheathe"]));
        assert!(is_bare_invocation(["sheathe", "--no-banner"]));
        assert!(!is_bare_invocation(["sheathe", "--help"]));
        assert!(!is_bare_invocation(["sheathe", "probe", "in.mp4"]));
        assert!(!is_bare_invocation(["sheathe", "--no-banner", "probe", "in.mp4"]));
    }

    fn long_help(subcommand: &str) -> String {
        let mut cmd = Cli::command();
        let sub = cmd.find_subcommand_mut(subcommand).expect(subcommand);
        let mut buf = Vec::new();
        sub.write_long_help(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn package_long_help_is_descriptive() {
        let help = long_help("package");
        for needle in [
            "Output:",
            "Presentation:",
            "Encryption:",
            "stream_descriptor",
            "--mpd_output",
            "--hls_master_playlist_output",
            "--protection_scheme",
            "--segment_duration",
        ] {
            assert!(help.contains(needle), "missing {needle:?} in package --help");
        }
    }

    #[test]
    fn probe_long_help_mentions_shaka_dump() {
        let help = long_help("probe");
        assert!(help.contains("dump_stream_info"), "{help}");
    }

    #[test]
    fn origin_long_help_documents_http_api() {
        let help = long_help("origin");
        assert!(help.contains("/package?input="), "{help}");
        assert!(help.contains("/health"), "{help}");
    }

    #[test]
    fn root_long_help_contrasts_shaka_descriptors() {
        let mut buf = Vec::new();
        Cli::command().write_long_help(&mut buf).unwrap();
        let help = String::from_utf8(buf).unwrap();
        assert!(help.contains("stream descriptor"), "{help}");
        assert!(help.contains("packager"), "{help}");
    }
}

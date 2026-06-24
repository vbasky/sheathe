//! The startup banner printed by the `sheathe` CLI.

/// ASCII-art wordmark, printed to stderr on an interactive run.
pub const BANNER: &str = r#"
        _                 _   _
   ___ | |__   ___  __ _ | |_| |__   ___
  / __|| '_ \ / _ \/ _` || __| '_ \ / _ \
  \__ \| | | |  __/ (_| || |_| | | |  __/
  |___/|_| |_|\___|\__,_| \__|_| |_|\___|
"#;

/// The tagline shown under the wordmark.
pub const TAGLINE: &str = "pure-Rust HLS / DASH / CMAF packager";

/// Print the banner, version, and tagline to stderr.
pub fn print() {
    eprint!("{BANNER}");
    eprintln!("  sheathe {}  —  {TAGLINE}\n", env!("CARGO_PKG_VERSION"));
}

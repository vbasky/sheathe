//! `sheathe` — pure-Rust HLS/DASH/CMAF media packager.
//!
//! `cargo install sheathe-cli` installs this binary; the CLI implementation
//! lives in [`sheathe_cli`].

fn main() -> anyhow::Result<()> {
    sheathe_cli::run()
}

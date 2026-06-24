//! `sheathe` — pure-Rust HLS/DASH/CMAF media packager.
//!
//! The canonical install target (`cargo install sheathe`); the CLI itself lives
//! in [`sheathe_cli`].

fn main() -> anyhow::Result<()> {
    sheathe_cli::run()
}

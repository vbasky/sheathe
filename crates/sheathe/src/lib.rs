//! # sheathe — pure-Rust HLS / DASH / CMAF media packager
//!
//! This is the **facade crate** for the sheathe workspace. It re-exports the
//! packaging pipeline ([`package`], [`probe`], [`serve_origin`]) and each
//! format crate as a module so you can depend on a single `sheathe` crate
//! instead of a dozen `sheathe-*` crates.
//!
//! The command-line tool lives in the separate `sheathe-cli` crate, which
//! installs a `sheathe` binary: `cargo install sheathe-cli`.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use sheathe::{package, PackageOptions};
//! use std::path::PathBuf;
//!
//! let opts = PackageOptions {
//!     out_dir: PathBuf::from("out"),
//!     dash: true,
//!     hls: true,
//!     segment_duration: 6.0,
//!     ..Default::default()
//! };
//! let output = package(&[PathBuf::from("movie.mp4")], &opts).unwrap();
//! ```
//!
//! ## Modules
//!
//! | Module | Crate | Purpose |
//! |--------|-------|---------|
//! | *top level* | `sheathe-package` | End-to-end `package` / `probe` / origin |
//! | [`core`] | `sheathe-core` | Media model: streams, samples, timing, errors |
//! | [`mp4`] | `sheathe-mp4` | ISO-BMFF / fMP4 / CMAF box writing + fragmentation |
//! | [`ts`] | `sheathe-ts` | MPEG-2 TS demux + mux |
//! | [`es`] | `sheathe-es` | Raw elementary stream demux |
//! | [`mkv`] | `sheathe-mkv` | WebM/Matroska (EBML) demux |
//! | [`text`] | `sheathe-text` | WebVTT + CEA-608/708 |
//! | [`dash`] | `sheathe-dash` | MPEG-DASH `.mpd` generation |
//! | [`hls`] | `sheathe-hls` | HLS master + media playlist generation |
//! | [`crypto`] | `sheathe-crypto` | Common Encryption (`cenc` / `cbcs`) |

pub use sheathe_package::*;

pub use sheathe_core as core;
pub use sheathe_crypto as crypto;
pub use sheathe_dash as dash;
pub use sheathe_es as es;
pub use sheathe_hls as hls;
pub use sheathe_mkv as mkv;
pub use sheathe_mp4 as mp4;
pub use sheathe_text as text;
pub use sheathe_ts as ts;

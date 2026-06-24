//! Core media model for the **sheathe** packager.
//!
//! This crate plays the role of Shaka Packager's `media/base`: the format-agnostic
//! abstractions every other crate is built on — elementary streams, samples,
//! timing, and the shared error type. It deliberately knows nothing about MP4,
//! DASH, or HLS; those live in [`sheathe-mp4`], [`sheathe-dash`], and
//! [`sheathe-hls`].
//!
//! [`sheathe-mp4`]: https://crates.io/crates/sheathe-mp4
//! [`sheathe-dash`]: https://crates.io/crates/sheathe-dash
//! [`sheathe-hls`]: https://crates.io/crates/sheathe-hls

mod error;
mod sample;
mod stream;
mod time;

pub use error::{Error, Result};
pub use sample::{Sample, SampleFlags};
pub use stream::{Codec, MediaKind, StreamInfo};
pub use time::{Scaled, Timescale};

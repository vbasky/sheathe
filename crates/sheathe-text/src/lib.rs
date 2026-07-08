//! Timed-text input for the **sheathe** packager.
//!
//! Parses WebVTT documents into a [`TextTrack`] of ISO/IEC 14496-30 (`wvtt`)
//! cue samples with a `wvtt`/`vttC` sample entry, ready for CMAF segmentation —
//! the read side for `.vtt` inputs.

mod cea608;
mod webvtt;

pub use cea608::{extract_cea608, extract_cea608_owned};
pub use webvtt::{TextTrack, webvtt};

/// Detect WebVTT by extension or the `WEBVTT` signature.
pub fn is_webvtt(path: &str, data: &[u8]) -> bool {
    if path.to_ascii_lowercase().ends_with(".vtt") {
        return true;
    }
    let head = &data[..data.len().min(16)];
    std::str::from_utf8(head).map(|s| s.trim_start().starts_with("WEBVTT")).unwrap_or(false)
}

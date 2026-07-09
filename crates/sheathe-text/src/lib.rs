//! Timed-text input for the **sheathe** packager.
//!
//! Three read paths, all producing [`TextTrack`]s of ISO/IEC 14496-30 cue
//! samples with a `wvtt`/`stpp` sample entry, ready for CMAF segmentation:
//!
//! - [`webvtt`] — parse a `.vtt` document.
//! - [`ttml`] — passthrough a TTML/IMSC document (`stpp` sample entry).
//! - [`extract_captions`] — recover CEA-608 (field 1 & 2) and CEA-708 (DTVCC)
//!   closed captions from H.264/H.265 SEI and decode them to WebVTT.

mod cea608;
mod cea708;
mod sei;
mod webvtt;

pub use webvtt::{TextTrack, ttml, webvtt};

/// Extract every CEA-608/708 caption track carried in a sequence of
/// `(pts_90k, annex_b_access_unit)` H.264 (or HEVC when `hevc`) video samples.
///
/// Returns one [`TextTrack`] per caption source found — CEA-608 CC1 (field 1),
/// CEA-608 CC3 (field 2), then one per CEA-708 DTVCC service — in that order.
/// Empty when the stream carries no captions.
pub fn extract_captions(samples: &[(u64, &[u8])], hevc: bool) -> Vec<TextTrack> {
    let triples = sei::cc_triples(samples, hevc);
    if triples.is_empty() {
        return Vec::new();
    }
    let mut tracks = Vec::new();
    if let Some(t) = cea608::decode_field(&triples, 0) {
        tracks.push(t);
    }
    if let Some(t) = cea608::decode_field(&triples, 1) {
        tracks.push(t);
    }
    tracks.extend(cea708::decode(&triples));
    tracks
}

/// Detect TTML by the `<tt` signature (handles XML declaration prefix).
pub fn is_ttml(data: &[u8]) -> bool {
    let head = &data[..data.len().min(128)];
    std::str::from_utf8(head).map(|s| s.contains("<tt")).unwrap_or(false)
}

/// Detect WebVTT by extension or the `WEBVTT` signature.
pub fn is_webvtt(path: &str, data: &[u8]) -> bool {
    if path.to_ascii_lowercase().ends_with(".vtt") {
        return true;
    }
    let head = &data[..data.len().min(16)];
    std::str::from_utf8(head).map(|s| s.trim_start().starts_with("WEBVTT")).unwrap_or(false)
}

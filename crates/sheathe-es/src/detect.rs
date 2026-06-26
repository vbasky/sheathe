//! Raw elementary stream detection from file extension and content.

use std::path::Path;

/// Kind of raw elementary stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// H.264 Annex B (`.h264`, `.264`, `.avc`).
    H264AnnexB,
    /// HEVC Annex B (`.hevc`, `.h265`, `.265`).
    HevcAnnexB,
    /// AAC with ADTS headers (`.aac`, `.adts`).
    AacAdts,
}

/// Guess stream kind from `path` and the first bytes of `data`.
pub fn detect(path: &str, data: &[u8]) -> Option<StreamKind> {
    extension_kind(path).or_else(|| sniff_content(data))
}

/// True when `data` looks like ISO-BMFF (MP4), not a raw elementary stream.
pub fn is_mp4(data: &[u8]) -> bool {
    data.len() >= 8 && &data[4..8] == b"ftyp"
}

fn extension_kind(path: &str) -> Option<StreamKind> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "h264" | "264" | "avc" => Some(StreamKind::H264AnnexB),
        "hevc" | "h265" | "265" => Some(StreamKind::HevcAnnexB),
        "aac" | "adts" => Some(StreamKind::AacAdts),
        _ => None,
    }
}

fn sniff_content(data: &[u8]) -> Option<StreamKind> {
    if is_adts(data) {
        return Some(StreamKind::AacAdts);
    }
    if !looks_like_annex_b(data) {
        return None;
    }
    if has_hevc_parameter_set(data) {
        Some(StreamKind::HevcAnnexB)
    } else if has_avc_parameter_set(data) {
        Some(StreamKind::H264AnnexB)
    } else {
        None
    }
}

fn is_adts(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xff && (data[1] & 0xf0) == 0xf0
}

fn looks_like_annex_b(data: &[u8]) -> bool {
    data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1])
}

fn has_avc_parameter_set(data: &[u8]) -> bool {
    scan_nal_types(data).any(|ty| ty == 7)
}

fn has_hevc_parameter_set(data: &[u8]) -> bool {
    scan_nal_types(data).any(|ty| ty == 32 || ty == 33)
}

fn scan_nal_types(data: &[u8]) -> impl Iterator<Item = u8> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        while i < data.len() {
            let (start, hevc) = if data[i..].starts_with(&[0, 0, 0, 1]) {
                (i + 4, true)
            } else if data[i..].starts_with(&[0, 0, 1]) {
                (i + 3, false)
            } else {
                i += 1;
                continue;
            };
            if start < data.len() {
                let ty = if hevc {
                    (data[start] >> 1) & 0x3f
                } else {
                    data[start] & 0x1f
                };
                i = start + 1;
                return Some(ty);
            }
            break;
        }
        None
    })
}
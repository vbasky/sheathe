//! RFC 6381 `codecs=` string derivation from sample-entry configuration boxes.
//!
//! Given a sample entry (e.g. `avc1`, `hvc1`, `mp4a`), locate its codec
//! configuration record (`avcC`, `hvcC`, `esds`) and format the profile/level
//! string players expect in DASH/HLS manifests (e.g. `avc1.640028`,
//! `hvc1.1.6.L93.B0`, `mp4a.40.2`). Returns `None` when the config is absent or
//! the codec isn't yet supported, in which case callers fall back to the fourcc.

use crate::box_reader::top_level;
use sheathe_core::MediaKind;

/// Byte length of the fixed `VisualSampleEntry` prefix before child boxes.
const VISUAL_PREFIX: usize = 78;
/// Byte length of the fixed `AudioSampleEntry` (v0) prefix before child boxes.
const AUDIO_PREFIX: usize = 28;

/// Derive the RFC 6381 codec string for one sample entry, if possible.
///
/// `entry_body` is the sample-entry box body (after its 8-byte header).
pub(crate) fn rfc6381(kind: MediaKind, fourcc: &[u8; 4], entry_body: &[u8]) -> Option<String> {
    let prefix = match kind {
        MediaKind::Video => VISUAL_PREFIX,
        MediaKind::Audio => AUDIO_PREFIX,
        MediaKind::Text => return None,
    };
    let children = entry_body.get(prefix..)?;
    match fourcc {
        b"avc1" | b"avc3" => avc_string(fourcc, find_child(children, b"avcC")?),
        b"hvc1" | b"hev1" => hevc_string(fourcc, find_child(children, b"hvcC")?),
        b"av01" => av1_string(find_child(children, b"av1C")?),
        b"mp4a" => mp4a_string(find_child(children, b"esds")?),
        _ => None,
    }
}

/// Find a child box body by type within a back-to-back box sequence.
fn find_child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    top_level(data)
        .flatten()
        .find(|b| &b.kind == kind)
        .map(|b| b.body)
}

/// NAL-unit length prefix size (bytes) for an AVC/HEVC sample entry, from
/// `avcC`/`hvcC`. Used to split samples into clear/protected subsamples for CENC.
pub(crate) fn nal_unit_length_size(fourcc: &[u8; 4], entry_body: &[u8]) -> Option<u8> {
    let children = entry_body.get(VISUAL_PREFIX..)?;
    let (config, offset) = match fourcc {
        b"avc1" | b"avc3" => (find_child(children, b"avcC")?, 4),
        b"hvc1" | b"hev1" => (find_child(children, b"hvcC")?, 21),
        _ => return None,
    };
    // lengthSizeMinusOne is the low two bits of that byte.
    Some((config.get(offset)? & 0x03) + 1)
}

/// `avc1.PPCCLL` — profile / constraint flags / level, hex, from `avcC`.
fn avc_string(fourcc: &[u8; 4], avcc: &[u8]) -> Option<String> {
    // avcC: configurationVersion(1), profile(1), profile_compat(1), level(1), …
    let (profile, compat, level) = (*avcc.get(1)?, *avcc.get(2)?, *avcc.get(3)?);
    let prefix = std::str::from_utf8(fourcc).ok()?;
    // RFC 6381 avc1 strings are conventionally lowercase (matches Shaka Packager).
    Some(format!("{prefix}.{profile:02x}{compat:02x}{level:02x}"))
}

/// `hvc1.A.B.C.D` from `hvcC` (ISO/IEC 14496-15).
fn hevc_string(fourcc: &[u8; 4], hvcc: &[u8]) -> Option<String> {
    // configurationVersion(1), [space(2)|tier(1)|profile_idc(5)](1),
    // compatibility_flags(4), constraint_flags(6), level_idc(1), …
    if hvcc.len() < 13 {
        return None;
    }
    let prefix = std::str::from_utf8(fourcc).ok()?;
    let b1 = hvcc[1];
    let profile_space = (b1 >> 6) & 0x3;
    let tier = (b1 >> 5) & 0x1;
    let profile_idc = b1 & 0x1f;
    let compat = u32::from_be_bytes([hvcc[2], hvcc[3], hvcc[4], hvcc[5]]);
    let constraint = &hvcc[6..12];
    let level = hvcc[12];

    let space = match profile_space {
        1 => "A",
        2 => "B",
        3 => "C",
        _ => "",
    };
    let tier_char = if tier == 0 { 'L' } else { 'H' };
    // Compatibility flags are bit-reversed then printed in hex (no leading zeros).
    let mut s = format!(
        "{prefix}.{space}{profile_idc}.{:X}.{tier_char}{level}",
        compat.reverse_bits()
    );

    // Constraint bytes: drop trailing zero bytes, hex, dot-separated.
    let last = constraint
        .iter()
        .rposition(|&b| b != 0)
        .map_or(0, |i| i + 1);
    for byte in &constraint[..last] {
        s.push_str(&format!(".{byte:02X}"));
    }
    Some(s)
}

/// `av01.P.LLT.BB` from `av1C` (profile, seq_level_idx, tier, bit depth).
fn av1_string(av1c: &[u8]) -> Option<String> {
    // byte0: marker+version; byte1: seq_profile(3) | seq_level_idx_0(5);
    // byte2: seq_tier_0(1) | high_bitdepth(1) | twelve_bit(1) | …
    let b1 = *av1c.get(1)?;
    let b2 = *av1c.get(2)?;
    let profile = b1 >> 5;
    let level_idx = b1 & 0x1f;
    let tier = if (b2 >> 7) & 1 == 0 { 'M' } else { 'H' };
    let high = (b2 >> 6) & 1;
    let twelve = (b2 >> 5) & 1;
    let depth = if twelve == 1 {
        12
    } else if high == 1 {
        10
    } else {
        8
    };
    Some(format!("av01.{profile}.{level_idx:02}{tier}.{depth:02}"))
}

/// `mp4a.OTI[.AOT]` from `esds` (e.g. `mp4a.40.2` for AAC-LC).
fn mp4a_string(esds: &[u8]) -> Option<String> {
    let mut p = 4; // skip full-box version + flags

    // ES_Descriptor (tag 0x03): ES_ID(2) + flags(1) [+ optional fields].
    let (tag, _, np) = read_descriptor(esds, p)?;
    if tag != 0x03 {
        return None;
    }
    p = np;
    let flags = *esds.get(p + 2)?;
    p += 3;
    if flags & 0x80 != 0 {
        p += 2; // dependsOn_ES_ID
    }
    if flags & 0x40 != 0 {
        let url_len = *esds.get(p)? as usize; // URLstring
        p += 1 + url_len;
    }
    if flags & 0x20 != 0 {
        p += 2; // OCR_ES_Id
    }

    // DecoderConfigDescriptor (tag 0x04): objectTypeIndication(1) + 12 bytes.
    let (tag, _, np) = read_descriptor(esds, p)?;
    if tag != 0x04 {
        return None;
    }
    p = np;
    let oti = *esds.get(p)?;
    p += 1 + 12; // OTI consumed + streamType/bufferSizeDB/max/avg bitrate

    let mut s = format!("mp4a.{oti:02X}");

    // DecoderSpecificInfo (tag 0x05): AudioSpecificConfig, AOT in top 5 bits.
    if oti == 0x40 {
        if let Some((tag, _, np)) = read_descriptor(esds, p) {
            if tag == 0x05 {
                if let Some(&asc0) = esds.get(np) {
                    let mut aot = u32::from(asc0 >> 3);
                    if aot == 31 {
                        // Escape value: AOT = 32 + next 6 bits.
                        if let Some(&asc1) = esds.get(np + 1) {
                            aot = 32 + ((u32::from(asc0 & 0x07) << 3) | u32::from(asc1 >> 5));
                        }
                    }
                    s = format!("mp4a.40.{aot}");
                }
            }
        }
    }
    Some(s)
}

/// Read an MPEG-4 descriptor header at `p`: returns (tag, length, payload_pos).
/// Length uses the expandable 7-bits-per-byte encoding (max 4 bytes).
fn read_descriptor(b: &[u8], mut p: usize) -> Option<(u8, usize, usize)> {
    let tag = *b.get(p)?;
    p += 1;
    let mut len = 0usize;
    for _ in 0..4 {
        let byte = *b.get(p)?;
        p += 1;
        len = (len << 7) | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some((tag, len, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avc_high_3_1() {
        // avcC: version=1, profile=0x64 (High), compat=0x00, level=0x1f (3.1)
        let avcc = [1u8, 0x64, 0x00, 0x1f];
        assert_eq!(avc_string(b"avc1", &avcc).as_deref(), Some("avc1.64001f"));
    }

    #[test]
    fn aac_lc() {
        // esds: ES_Descriptor > DecoderConfigDescriptor(OTI=0x40) > DSI(ASC=0x12,0x10)
        let esds = [
            0, 0, 0, 0, // version + flags
            0x03, 0x19, 0x00, 0x00, 0x00, // ES_Descriptor: tag,len,ES_ID,flags
            0x04, 0x11, 0x40, // DecoderConfigDescriptor: tag,len,OTI
            0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // streamType + buffer/bitrates (12)
            0x05, 0x02, 0x12, 0x10, // DecoderSpecificInfo: tag,len,ASC (AAC-LC)
        ];
        assert_eq!(mp4a_string(&esds).as_deref(), Some("mp4a.40.2"));
    }

    #[test]
    fn hevc_main_l93() {
        // hvcC: version, profile_idc=1, compat=0x60000000, constraint=0x90.., level=93
        let hvcc = [1u8, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0, 0, 0, 0, 0, 0x5d];
        assert_eq!(
            hevc_string(b"hvc1", &hvcc).as_deref(),
            Some("hvc1.1.6.L93.90")
        );
    }

    #[test]
    fn av1_main_l4_8bit() {
        // av1C: marker+version, seq_profile=0|seq_level_idx=4, tier=0/8-bit.
        let av1c = [0x81u8, 0x04, 0x00, 0x00];
        assert_eq!(av1_string(&av1c).as_deref(), Some("av01.0.04M.08"));
    }
}

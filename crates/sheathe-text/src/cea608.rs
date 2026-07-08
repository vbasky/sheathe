//! CEA-608 closed-caption extraction from H.264/H.265 SEI, decoded to WebVTT.
//!
//! Pulls `cc_data` (field 1) out of `user_data_registered_itu_t_t35` (`GA94`)
//! SEI messages and runs a CEA-608 decoder covering the pop-on and roll-up
//! paths plus the North-American basic/special character sets. CEA-708 DTVCC
//! service decoding is out of scope (those packets are skipped).

use crate::webvtt::{TextTrack, webvtt};
use sheathe_core::Result;

/// One coded caption byte pair tagged with the access unit's presentation time.
struct CcPair {
    pts_ms: u64,
    b0: u8,
    b1: u8,
}

/// Extract CEA-608 captions from a sequence of `(pts_90k, annex_b_au)` video
/// samples and return a WebVTT text track, or `None` if no captions are present.
pub fn extract_cea608(samples: &[(u64, &[u8])], hevc: bool) -> Option<TextTrack> {
    let mut pairs = Vec::new();
    for &(pts, au) in samples {
        for nal in split_nals(au) {
            if is_sei(nal, hevc) {
                collect_cc(sei_rbsp(nal, hevc), pts / 90, &mut pairs);
            }
        }
    }
    if pairs.is_empty() {
        return None;
    }
    let doc = decode_608(&pairs);
    webvtt(&doc).ok()
}

/// Convenience over an owned sample list (as produced by the demuxers).
pub fn extract_cea608_owned(samples: &[(u64, Vec<u8>)], hevc: bool) -> Result<Option<TextTrack>> {
    let refs: Vec<(u64, &[u8])> = samples.iter().map(|(p, d)| (*p, d.as_slice())).collect();
    Ok(extract_cea608(&refs, hevc))
}

// ---- NAL / SEI plumbing --------------------------------------------------

/// Split Annex B data into NAL units (payloads without start codes).
fn split_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    let mut start = None;
    while i + 3 <= data.len() {
        let sc3 = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1;
        if sc3 {
            if let Some(s) = start {
                nals.push(&data[s..i]);
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        nals.push(&data[s..]);
    }
    nals
}

fn is_sei(nal: &[u8], hevc: bool) -> bool {
    match nal.first() {
        None => false,
        Some(&b) if hevc => ((b >> 1) & 0x3f) == 39 || ((b >> 1) & 0x3f) == 40, // PREFIX/SUFFIX SEI
        Some(&b) => (b & 0x1f) == 6,
    }
}

/// The SEI RBSP after the NAL header (with emulation-prevention bytes removed).
fn sei_rbsp(nal: &[u8], hevc: bool) -> Vec<u8> {
    let header = if hevc { 2 } else { 1 };
    let mut out = Vec::with_capacity(nal.len());
    let mut i = header;
    while i < nal.len() {
        if i + 2 < nal.len() && nal[i] == 0 && nal[i + 1] == 0 && nal[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
        } else {
            out.push(nal[i]);
            i += 1;
        }
    }
    out
}

/// Walk SEI messages, decode `GA94` cc_data, and append field-1 byte pairs.
fn collect_cc(rbsp: Vec<u8>, pts_ms: u64, out: &mut Vec<CcPair>) {
    let mut i = 0;
    while i < rbsp.len() {
        // payloadType and payloadSize are 0xFF-continued.
        let mut payload_type = 0usize;
        while i < rbsp.len() && rbsp[i] == 0xff {
            payload_type += 255;
            i += 1;
        }
        if i >= rbsp.len() {
            break;
        }
        payload_type += usize::from(rbsp[i]);
        i += 1;
        let mut payload_size = 0usize;
        while i < rbsp.len() && rbsp[i] == 0xff {
            payload_size += 255;
            i += 1;
        }
        if i >= rbsp.len() {
            break;
        }
        payload_size += usize::from(rbsp[i]);
        i += 1;
        let end = (i + payload_size).min(rbsp.len());
        let payload = &rbsp[i..end];
        if payload_type == 4 {
            parse_t35(payload, pts_ms, out);
        }
        i = end;
        if rbsp.get(i) == Some(&0x80) {
            break; // rbsp_trailing_bits
        }
    }
}

fn parse_t35(p: &[u8], pts_ms: u64, out: &mut Vec<CcPair>) {
    // country 0xB5, provider 0x0031, user_id "GA94", type 0x03.
    if p.len() < 10 || p[0] != 0xb5 || &p[3..7] != b"GA94" || p[7] != 0x03 {
        return;
    }
    let cc_count = usize::from(p[8] & 0x1f);
    let mut idx = 10; // skip em_data byte
    for _ in 0..cc_count {
        if idx + 3 > p.len() {
            break;
        }
        let flag = p[idx];
        let cc_valid = flag & 0x04 != 0;
        let cc_type = flag & 0x03;
        let b0 = p[idx + 1];
        let b1 = p[idx + 2];
        idx += 3;
        if cc_valid && cc_type == 0 {
            // CEA-608 field 1.
            out.push(CcPair { pts_ms, b0, b1 });
        }
    }
}

// ---- CEA-608 decode ------------------------------------------------------

/// Decode field-1 608 pairs into a WebVTT document.
fn decode_608(pairs: &[CcPair]) -> String {
    let mut cues: Vec<(u64, u64, String)> = Vec::new();
    let mut back = String::new(); // pop-on: caption being loaded
    let mut display: Option<(u64, String)> = None; // (start_ms, text) currently on screen
    let mut rollup = false;
    let mut last_ctrl: Option<(u8, u8)> = None;

    for p in pairs {
        let b0 = p.b0 & 0x7f; // strip odd parity
        let b1 = p.b1 & 0x7f;

        if (0x10..=0x1f).contains(&b0) {
            // Control code — doubled codes are deduplicated.
            if last_ctrl == Some((b0, b1)) {
                last_ctrl = None;
                continue;
            }
            last_ctrl = Some((b0, b1));

            let misc = (b0 == 0x14 || b0 == 0x1c) && (0x20..=0x2f).contains(&b1);
            if misc {
                match b1 {
                    0x20 => {
                        // RCL: resume caption loading (pop-on).
                        rollup = false;
                        back.clear();
                    }
                    0x25..=0x27 => rollup = true, // RU2/RU3/RU4
                    0x2c => {
                        // EDM: erase displayed memory → close current cue.
                        if let Some((start, text)) = display.take() {
                            push_cue(&mut cues, start, p.pts_ms, text);
                        }
                    }
                    0x2d => {
                        // CR: carriage return (roll-up) → emit the line.
                        if rollup && !back.is_empty() {
                            push_cue(&mut cues, p.pts_ms, p.pts_ms + 2000, back.clone());
                            back.clear();
                        }
                    }
                    0x2e => back.clear(), // ENM: erase non-displayed memory
                    0x2f => {
                        // EOC: end of caption → flip back buffer to display.
                        if let Some((start, text)) = display.take() {
                            push_cue(&mut cues, start, p.pts_ms, text);
                        }
                        display = Some((p.pts_ms, std::mem::take(&mut back)));
                    }
                    _ => {}
                }
            } else if b0 == 0x11 && (0x30..=0x3f).contains(&b1) {
                back.push(special_char(b1));
            } else if is_pac(b0, b1) {
                // Preamble address code → new row.
                if !back.is_empty() && !back.ends_with('\n') {
                    back.push('\n');
                }
            }
            // Mid-row and extended codes are ignored (styling only).
            continue;
        }

        last_ctrl = None;
        if b0 >= 0x20 {
            back.push(basic_char(b0));
        }
        if b1 >= 0x20 {
            back.push(basic_char(b1));
        }
    }

    // Flush a still-displayed caption.
    if let Some((start, text)) = display.take() {
        let end = pairs.last().map(|p| p.pts_ms + 2000).unwrap_or(start + 2000);
        push_cue(&mut cues, start, end, text);
    }

    render_webvtt(&cues)
}

fn push_cue(cues: &mut Vec<(u64, u64, String)>, start: u64, end: u64, text: String) {
    let text = text.trim_matches('\n').trim().to_string();
    if !text.is_empty() && end > start {
        cues.push((start, end, text));
    }
}

fn is_pac(b0: u8, b1: u8) -> bool {
    (0x10..=0x17).contains(&b0) && (0x40..=0x7f).contains(&b1)
}

/// CEA-608 basic character set (ASCII with a few substitutions).
fn basic_char(c: u8) -> char {
    match c {
        0x2a => 'á',
        0x5c => 'é',
        0x5e => 'í',
        0x5f => 'ó',
        0x60 => 'ú',
        0x7b => 'ç',
        0x7c => '÷',
        0x7d => 'Ñ',
        0x7e => 'ñ',
        0x7f => '█',
        _ => c as char,
    }
}

/// CEA-608 special characters (0x11, 0x30..0x3F).
fn special_char(c: u8) -> char {
    match c {
        0x30 => '®',
        0x31 => '°',
        0x32 => '½',
        0x33 => '¿',
        0x34 => '™',
        0x35 => '¢',
        0x36 => '£',
        0x37 => '♪',
        0x38 => 'à',
        0x39 => ' ',
        0x3a => 'è',
        0x3b => 'â',
        0x3c => 'ê',
        0x3d => 'î',
        0x3e => 'ô',
        0x3f => 'û',
        _ => ' ',
    }
}

fn render_webvtt(cues: &[(u64, u64, String)]) -> String {
    let mut out = String::from("WEBVTT\n");
    for (start, end, text) in cues {
        out.push_str(&format!("\n{} --> {}\n{}\n", fmt_ts(*start), fmt_ts(*end), text));
    }
    out
}

fn fmt_ts(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let milli = ms % 1000;
    format!("{h:02}:{m:02}:{s:02}.{milli:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap a T35/GA94 cc_data payload with `cc_count` field-1 pairs into an
    /// Annex B H.264 SEI access unit.
    fn sei_au(pairs: &[(u8, u8)]) -> Vec<u8> {
        let mut cc = vec![0xb5, 0x00, 0x31]; // country + provider
        cc.extend_from_slice(b"GA94");
        cc.push(0x03); // user_data_type_code
        cc.push(0xc0 | pairs.len() as u8); // process_cc=1, cc_count
        cc.push(0xff); // em_data
        for &(a, b) in pairs {
            cc.push(0xfc); // marker(111111) + cc_valid(1) + cc_type(00 = field 1)
            cc.push(a);
            cc.push(b);
        }
        // SEI: payloadType 4, size, payload, trailing 0x80.
        let mut rbsp = vec![0x04, cc.len() as u8];
        rbsp.extend_from_slice(&cc);
        rbsp.push(0x80);
        let mut au = vec![0, 0, 1, 0x06]; // start code + SEI NAL header
        au.extend_from_slice(&rbsp);
        au
    }

    /// Odd-parity is stripped on decode, so raw 7-bit values are fine in tests.
    fn ctrl(b0: u8, b1: u8) -> (u8, u8) {
        (b0, b1)
    }

    #[test]
    fn extracts_pop_on_caption() {
        // RCL, PAC(row), 'H','I', EOC @1s ; EDM @3s.
        let au1 = sei_au(&[ctrl(0x14, 0x20), (0x48, 0x49), ctrl(0x14, 0x2f)]);
        let au2 = sei_au(&[ctrl(0x14, 0x2c)]);
        let samples = [(90_000u64, au1.as_slice()), (270_000u64, au2.as_slice())];
        let track = extract_cea608(&samples, false).expect("captions");
        // One cue "HI" spanning 1s..3s → one non-empty cue + a leading gap sample.
        assert!(track.samples.iter().any(|s| s.data.windows(4).any(|w| w == b"payl")));
        // The decoded WebVTT should contain "HI".
        let has_hi = track.samples.iter().any(|s| {
            let p = s.data.windows(2).position(|w| w == b"HI");
            p.is_some()
        });
        assert!(has_hi);
    }

    #[test]
    fn no_sei_no_track() {
        let au = vec![0u8, 0, 1, 0x65, 0x88]; // an IDR slice, no SEI
        assert!(extract_cea608(&[(0, au.as_slice())], false).is_none());
    }

    #[test]
    fn timestamp_formatting() {
        assert_eq!(fmt_ts(3_723_500), "01:02:03.500");
        assert_eq!(fmt_ts(1000), "00:00:01.000");
    }
}

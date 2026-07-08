//! CEA-608 ("line 21") caption decoding for a single field.
//!
//! Covers the pop-on and roll-up paths plus the North-American basic/special
//! character sets. Field 1 (`cc_type` 0) carries CC1; field 2 (`cc_type` 1)
//! carries CC3 — the same decoder runs over either.

use crate::sei::Triple;
use crate::webvtt::{TextTrack, render_cues, webvtt};

/// Decode one CEA-608 field (`field` = 0 or 1) into a WebVTT track, or `None`
/// if that field carries no caption text.
pub(crate) fn decode_field(triples: &[Triple], field: u8) -> Option<TextTrack> {
    let pairs: Vec<&Triple> = triples.iter().filter(|t| t.cc_type == field).collect();
    if pairs.is_empty() {
        return None;
    }
    let cues = decode(&pairs);
    if cues.is_empty() {
        return None;
    }
    webvtt(&render_cues(&cues)).ok()
}

/// Run the 608 state machine over one field's byte pairs → caption cues.
fn decode(pairs: &[&Triple]) -> Vec<(u64, u64, String)> {
    let mut cues: Vec<(u64, u64, String)> = Vec::new();
    let mut back = String::new(); // pop-on: caption being loaded
    let mut display: Option<(u64, String)> = None; // (start_ms, text) on screen
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
            } else if is_pac(b0, b1) && !back.is_empty() && !back.ends_with('\n') {
                back.push('\n'); // preamble address code → new row
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
    cues
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sei::cc_triples;
    use crate::sei::test_support::sei_au;

    #[test]
    fn decodes_pop_on_field1() {
        // field 1: RCL, PAC, "HI", EOC @1s ; EDM @3s.
        let au1 = sei_au(&[(0, 0x14, 0x20), (0, 0x48, 0x49), (0, 0x14, 0x2f)]);
        let au2 = sei_au(&[(0, 0x14, 0x2c)]);
        let triples = cc_triples(&[(90_000, au1.as_slice()), (270_000, au2.as_slice())], false);
        let track = decode_field(&triples, 0).expect("cc1");
        assert!(track.samples.iter().any(|s| s.data.windows(2).any(|w| w == b"HI")));
    }

    #[test]
    fn decodes_field2_independently() {
        // Same caption but tagged field 2 → CC3.
        let au1 = sei_au(&[(1, 0x14, 0x20), (1, 0x4f, 0x4b), (1, 0x14, 0x2f)]);
        let au2 = sei_au(&[(1, 0x14, 0x2c)]);
        let triples = cc_triples(&[(90_000, au1.as_slice()), (270_000, au2.as_slice())], false);
        assert!(decode_field(&triples, 0).is_none()); // nothing on field 1
        let track = decode_field(&triples, 1).expect("cc3");
        assert!(track.samples.iter().any(|s| s.data.windows(2).any(|w| w == b"OK")));
    }
}

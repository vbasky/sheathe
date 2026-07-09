//! CEA-608 ("line 21") caption decoding for a single field.
//!
//! Covers the pop-on and roll-up paths plus the North-American basic/special
//! character sets. Field 1 (`cc_type` 0) carries CC1; field 2 (`cc_type` 1)
//! carries CC3 — the same decoder runs over either.

use crate::sei::Triple;
use crate::webvtt::{TextTrack, render_positioned_cues, webvtt};
use std::collections::BTreeMap;

/// A caption cue: `(start_ms, end_ms, text, optional WebVTT setting)`.
type Cue = (u64, u64, String, Option<String>);

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
    webvtt(&render_positioned_cues(&cues)).ok()
}

/// Run the 608 state machine over one field's byte pairs → positioned cues.
///
/// Captions are row-addressed: characters load into the row selected by the most
/// recent PAC, and each on-screen row becomes its own cue at the WebVTT line
/// position that row maps to (matching ccextractor's safe-area layout).
fn decode(pairs: &[&Triple]) -> Vec<Cue> {
    let mut cues: Vec<Cue> = Vec::new();
    let mut rows: BTreeMap<u8, String> = BTreeMap::new(); // non-displayed buffer
    let mut display: Option<(u64, BTreeMap<u8, String>)> = None; // on-screen
    let mut cur_row = 15u8;
    let mut rollup = false;
    let mut last_ctrl: Option<(u8, u8)> = None;

    for p in pairs {
        let b0 = p.b0 & 0x7f; // strip odd parity
        let b1 = p.b1 & 0x7f;

        if (0x10..=0x1f).contains(&b0) {
            if last_ctrl == Some((b0, b1)) {
                last_ctrl = None; // doubled control code
                continue;
            }
            last_ctrl = Some((b0, b1));

            let misc = (b0 == 0x14 || b0 == 0x1c) && (0x20..=0x2f).contains(&b1);
            if misc {
                match b1 {
                    0x20 => rollup = false, // RCL (does not clear the buffer)
                    0x25..=0x27 => {
                        rollup = true; // RU2/RU3/RU4
                        cur_row = 15;
                    }
                    0x2c => {
                        // EDM: erase displayed memory → close its cues.
                        if let Some((start, map)) = display.take() {
                            emit_rows(&mut cues, start, p.pts_ms, map);
                        }
                    }
                    0x2d => {
                        // CR: carriage return (roll-up) → emit the current row.
                        if rollup {
                            let text = rows.remove(&cur_row).unwrap_or_default();
                            if !text.trim().is_empty() {
                                cues.push((
                                    p.pts_ms,
                                    p.pts_ms + 2000,
                                    text.trim().to_string(),
                                    Some(line_setting(cur_row)),
                                ));
                            }
                            rows.clear();
                        }
                    }
                    0x2e => rows.clear(), // ENM: erase non-displayed memory
                    0x2f => {
                        // EOC: flip the non-displayed buffer to the display.
                        if let Some((start, map)) = display.take() {
                            emit_rows(&mut cues, start, p.pts_ms, map);
                        }
                        display = Some((p.pts_ms, std::mem::take(&mut rows)));
                    }
                    _ => {}
                }
            } else if b0 == 0x11 && (0x30..=0x3f).contains(&b1) {
                rows.entry(cur_row).or_default().push(special_char(b1));
            } else if is_pac(b0, b1) {
                cur_row = pac_row(b0, b1); // preamble address code → row
                rows.entry(cur_row).or_default();
            }
            continue;
        }

        last_ctrl = None;
        let row = rows.entry(cur_row).or_default();
        if b0 >= 0x20 {
            row.push(basic_char(b0));
        }
        if b1 >= 0x20 {
            row.push(basic_char(b1));
        }
    }

    if let Some((start, map)) = display.take() {
        let end = pairs.last().map(|p| p.pts_ms + 2000).unwrap_or(start + 2000);
        emit_rows(&mut cues, start, end, map);
    }
    cues
}

/// Emit each non-empty row of a displayed caption as its own positioned cue.
fn emit_rows(cues: &mut Vec<Cue>, start: u64, end: u64, rows: BTreeMap<u8, String>) {
    if end <= start {
        return;
    }
    for (row, text) in rows {
        let text = text.trim().to_string();
        if !text.is_empty() {
            cues.push((start, end, text, Some(line_setting(row))));
        }
    }
}

/// CEA-608 PAC (channel 1) → on-screen row number (1–15).
fn pac_row(b0: u8, b1: u8) -> u8 {
    let base = match b0 {
        0x11 => 1,
        0x12 => 3,
        0x15 => 5,
        0x16 => 7,
        0x17 => 9,
        0x10 => 11,
        0x13 => 12,
        0x14 => 14,
        _ => 15,
    };
    if b0 != 0x10 && b1 >= 0x60 { base + 1 } else { base }
}

/// WebVTT `line:` setting for a 608 row, using the same safe-area map as
/// ccextractor: row 1 → 10%, row 15 → 84.66% (linear, 16/3 % per row).
fn line_setting(row: u8) -> String {
    let pct = 10.0 + f64::from(row.saturating_sub(1)) * (16.0 / 3.0);
    let trunc = (pct * 100.0).floor() / 100.0; // ccextractor truncates to 2 dp
    let mut s = format!("{trunc:.2}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    format!("line:{s}%")
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

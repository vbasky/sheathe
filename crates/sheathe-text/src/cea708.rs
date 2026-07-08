//! CEA-708 (DTVCC) caption decoding.
//!
//! Reassembles DTVCC transport packets from the `cc_type` 2/3 byte stream,
//! splits them into service blocks, and runs a code-space interpreter (C0/C1/
//! G0/G1) over each service. A window model (define / display / hide / toggle /
//! clear / delete + text) turns the command stream into timed cues. Pen and
//! window *styling* commands are length-skipped but not rendered.

use crate::sei::Triple;
use crate::webvtt::{TextTrack, render_cues, webvtt};
use std::collections::BTreeMap;

/// Decode all DTVCC services present into one WebVTT track per service.
pub(crate) fn decode(triples: &[Triple]) -> Vec<TextTrack> {
    let packets = reassemble(triples);
    if packets.is_empty() {
        return Vec::new();
    }

    let mut services: BTreeMap<u8, Service> = BTreeMap::new();
    let mut last_pts = 0u64;
    for pkt in &packets {
        last_pts = pkt.pts_ms;
        for (service_number, block) in service_blocks(&pkt.data) {
            services.entry(service_number).or_default().feed(block, pkt.pts_ms);
        }
    }

    let mut tracks = Vec::new();
    // Deterministic order (service 1 first).
    for (_num, mut svc) in services {
        svc.flush(last_pts + 2000);
        if svc.cues.is_empty() {
            continue;
        }
        if let Ok(track) = webvtt(&render_cues(&svc.cues)) {
            tracks.push(track);
        }
    }
    tracks
}

// ---- DTVCC packet reassembly --------------------------------------------

struct Packet {
    pts_ms: u64,
    data: Vec<u8>,
}

/// Rebuild DTVCC packets from the interleaved `cc_type` 3 (start) / 2 (cont.)
/// byte pairs, trimming each to its declared `packet_size`.
fn reassemble(triples: &[Triple]) -> Vec<Packet> {
    let mut packets = Vec::new();
    let mut cur: Option<Packet> = None;
    for t in triples {
        match t.cc_type {
            3 => {
                if let Some(p) = cur.take() {
                    packets.push(finalize(p));
                }
                cur = Some(Packet { pts_ms: t.pts_ms, data: vec![t.b0, t.b1] });
            }
            2 => {
                if let Some(p) = cur.as_mut() {
                    p.data.push(t.b0);
                    p.data.push(t.b1);
                }
            }
            _ => {}
        }
    }
    if let Some(p) = cur.take() {
        packets.push(finalize(p));
    }
    packets
}

/// Trim a reassembled packet to its `packet_size_code` length.
fn finalize(mut p: Packet) -> Packet {
    if let Some(&first) = p.data.first() {
        let size_code = first & 0x3f;
        let len = if size_code == 0 { 128 } else { usize::from(size_code) * 2 };
        p.data.truncate(len);
    }
    p
}

/// Iterate `(service_number, block_data)` from a DTVCC packet body.
fn service_blocks(packet: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut i = 1; // skip packet header byte
    while i < packet.len() {
        let header = packet[i];
        i += 1;
        let mut service_number = header >> 5;
        let block_size = usize::from(header & 0x1f);
        if service_number == 7 {
            // Extended service number in a following byte.
            if i >= packet.len() {
                break;
            }
            service_number = packet[i] & 0x3f;
            i += 1;
        }
        if service_number == 0 {
            break; // NULL service block → padding
        }
        let end = (i + block_size).min(packet.len());
        out.push((service_number, &packet[i..end]));
        i = end;
    }
    out
}

// ---- Per-service window model -------------------------------------------

#[derive(Default, Clone)]
struct Window {
    defined: bool,
    visible: bool,
    start_ms: u64,
    text: String,
}

#[derive(Default)]
struct Service {
    windows: [Window; 8],
    current: usize,
    cues: Vec<(u64, u64, String)>,
}

impl Service {
    /// Process one service block's command stream.
    fn feed(&mut self, block: &[u8], pts: u64) {
        let mut i = 0;
        while i < block.len() {
            let b = block[i];
            match b {
                0x00 => i += 1, // NUL
                0x03 => i += 1, // ETX (end of text)
                0x08 => {
                    // BS (backspace)
                    self.windows[self.current].text.pop();
                    i += 1;
                }
                0x0c => {
                    // FF (form feed → clear window)
                    self.windows[self.current].text.clear();
                    i += 1;
                }
                0x0d => {
                    // CR
                    self.windows[self.current].text.push('\n');
                    i += 1;
                }
                0x0e => i += 1,                                           // HCR
                0x01 | 0x02 | 0x04..=0x07 | 0x09..=0x0b | 0x0f => i += 1, // other C0 1-byte
                0x10 => i += 2,        // EXT1 (extended char, skip)
                0x11..=0x17 => i += 2, // C0 2-byte
                0x18..=0x1f => i += 3, // C0 3-byte (P16, etc.)
                0x20..=0x7f => {
                    // G0 text
                    self.put(if b == 0x7f { '♪' } else { b as char });
                    i += 1;
                }
                0x80..=0x9f => i += self.c1(&block[i..], pts), // C1 command
                0xa0..=0xff => {
                    // G1 Latin-1 text
                    self.put(char::from(b));
                    i += 1;
                }
            }
        }
    }

    /// Handle one C1 command; return its total byte length (for advancing).
    fn c1(&mut self, cmd: &[u8], pts: u64) -> usize {
        let b = cmd[0];
        let arg = |n: usize| cmd.get(n).copied().unwrap_or(0);
        match b {
            0x80..=0x87 => {
                // CWx: set current window.
                self.current = usize::from(b & 0x07);
                1
            }
            0x88 => {
                // CLW: clear windows (bitmap).
                self.for_each_window(arg(1), pts, |w, pts, cues| {
                    emit(w, pts, cues);
                    w.text.clear();
                    w.start_ms = pts;
                });
                2
            }
            0x89 => {
                // DSW: display windows.
                self.for_each_window(arg(1), pts, |w, pts, _| {
                    if !w.visible {
                        w.visible = true;
                        w.start_ms = pts;
                    }
                });
                2
            }
            0x8a => {
                // HDW: hide windows.
                self.for_each_window(arg(1), pts, |w, pts, cues| {
                    emit(w, pts, cues);
                    w.visible = false;
                });
                2
            }
            0x8b => {
                // TGW: toggle windows.
                self.for_each_window(arg(1), pts, |w, pts, cues| {
                    if w.visible {
                        emit(w, pts, cues);
                        w.visible = false;
                    } else {
                        w.visible = true;
                        w.start_ms = pts;
                    }
                });
                2
            }
            0x8c => {
                // DLW: delete windows.
                self.for_each_window(arg(1), pts, |w, pts, cues| {
                    emit(w, pts, cues);
                    *w = Window::default();
                });
                2
            }
            0x8d => 2, // DLY
            0x8e => 1, // DLC
            0x8f => {
                // RST: reset — emit visible windows, wipe all.
                for w in &mut self.windows {
                    emit(w, pts, &mut self.cues);
                    *w = Window::default();
                }
                1
            }
            0x90 => 3,                                          // SPA SetPenAttributes
            0x91 => 4,                                          // SPC SetPenColor
            0x92 => 3,                                          // SPL SetPenLocation
            0x93..=0x96 => [4, 5, 6, 1][usize::from(b - 0x93)], // reserved
            0x97 => 5,                                          // SWA SetWindowAttributes
            0x98..=0x9f => {
                // DFx: define window (6 args); arg0 bit 0x20 = visible.
                let n = usize::from(b & 0x07);
                let visible = arg(1) & 0x20 != 0;
                let w = &mut self.windows[n];
                if !w.defined {
                    w.text.clear();
                }
                w.defined = true;
                w.visible = visible;
                w.start_ms = pts;
                self.current = n;
                7
            }
            _ => 1,
        }
    }

    fn put(&mut self, ch: char) {
        let w = &mut self.windows[self.current];
        if w.defined {
            w.text.push(ch);
        }
    }

    /// Apply `f` to every window selected by `bitmap`.
    fn for_each_window(
        &mut self,
        bitmap: u8,
        pts: u64,
        f: impl Fn(&mut Window, u64, &mut Vec<(u64, u64, String)>),
    ) {
        for (i, w) in self.windows.iter_mut().enumerate() {
            if bitmap & (1 << i) != 0 {
                f(w, pts, &mut self.cues);
            }
        }
    }

    /// Emit any still-visible windows at end of stream.
    fn flush(&mut self, pts: u64) {
        for w in &mut self.windows {
            emit(w, pts, &mut self.cues);
        }
    }
}

/// If `w` is visible with text, push a cue `[start, pts]` and clear its text.
fn emit(w: &mut Window, pts: u64, cues: &mut Vec<(u64, u64, String)>) {
    if w.visible {
        let text = w.text.trim_matches('\n').trim().to_string();
        if !text.is_empty() && pts > w.start_ms {
            cues.push((w.start_ms, pts, text));
            w.text.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sei::cc_triples;
    use crate::sei::test_support::sei_au;

    /// A DTVCC packet as `cc_type` 3/2 triples: DefineWindow 0 (visible) + "HI".
    fn dtvcc_hi() -> Vec<u8> {
        // block: DF0(0x98) [0x20 visible,0,0,0,0,0] 'H' 'I' → 9 bytes.
        // svc header: service 1, size 9 → 0x29. packet header: size_code 6 → 12 bytes.
        // packet: [hdr=0x06][svc=0x29] [DF0=0x98][6 args: 0x20,0,0,0,0,0]['H','I'][pad]
        let triples = [
            (3u8, 0x06, 0x29), // packet hdr + service-block hdr (service 1, size 9)
            (2, 0x98, 0x20),   // DF0 + arg0 (visible bit 0x20)
            (2, 0x00, 0x00),   // arg1, arg2
            (2, 0x00, 0x00),   // arg3, arg4
            (2, 0x00, 0x48),   // arg5, 'H'
            (2, 0x49, 0x00),   // 'I', pad
        ];
        sei_au(&triples)
    }

    #[test]
    fn decodes_dtvcc_window_text() {
        let au = dtvcc_hi();
        let triples = cc_triples(&[(90_000, au.as_slice())], false);
        let tracks = decode(&triples);
        assert_eq!(tracks.len(), 1);
        let t = &tracks[0];
        assert!(t.samples.iter().any(|s| s.data.windows(2).any(|w| w == b"HI")));
    }

    #[test]
    fn no_dtvcc_no_tracks() {
        // Only 608 field-1 triples present → no 708 output.
        let au = sei_au(&[(0, 0x48, 0x49)]);
        let triples = cc_triples(&[(0, au.as_slice())], false);
        assert!(decode(&triples).is_empty());
    }

    #[test]
    fn service_block_split() {
        let pkt = [0x06, 0x29, 0x98, 0x20, 0, 0, 0, 0, 0, 0x48, 0x49, 0x00];
        let blocks = service_blocks(&pkt);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, 1); // service 1
    }
}

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
    for (_num, mut svc) in services {
        svc.flush(last_pts + 2000);
        if svc.cues.is_empty() {
            continue;
        }
        let style_block: String = if svc.styles.is_empty() {
            String::new()
        } else {
            let mut s = "\nSTYLE\n".to_string();
            for (_, css) in &svc.styles {
                s.push_str(css);
                s.push('\n');
            }
            s
        };
        let webvtt_text = format!("WEBVTT{}\n{}", style_block, render_cues(&svc.cues));
        if let Ok(track) = webvtt(&webvtt_text) {
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
    /// Foreground colour in 2-bit-per-channel (0-3 each), set by SPC.
    pen_fg: Option<(u8, u8, u8)>,
    /// Background colour, set by SPC.
    pen_bg: Option<(u8, u8, u8)>,
    #[allow(dead_code)]
    italic: bool,
    #[allow(dead_code)]
    underline: bool,
}

#[derive(Default)]
struct Service {
    windows: [Window; 8],
    current: usize,
    cues: Vec<(u64, u64, String)>,
    /// Accumulated unique CSS styles (class_name → colour) for the STYLE block.
    styles: Vec<(String, String)>,
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
            0x90 => {
                // SPA SetPenAttributes: pen_size(2) | font_style(3) | italics(1) | underline(1) | text_type(1)
                self.style_pool(Some((arg(1), arg(2))));
                3
            }
            0x91 => {
                // SPC SetPenColor: fg_colour_3bytes(6bits each) | bg_3bytes | edge(3) | edge_colour(6)
                let fg_r = arg(1) >> 6;
                let fg_g = (arg(1) >> 4) & 3;
                let fg_b = (arg(1) >> 2) & 3;
                let bg_r = arg(2) >> 6;
                let bg_g = (arg(2) >> 4) & 3;
                let bg_b = (arg(2) >> 2) & 3;
                let w = &mut self.windows[self.current];
                w.pen_fg = Some((fg_r, fg_g, fg_b));
                w.pen_bg = Some((bg_r, bg_g, bg_b));
                4
            }
            0x92 => 3, // SPL SetPenLocation: row(4) | column(4) — positioning handled by other means
            0x93..=0x96 => [4, 5, 6, 1][usize::from(b - 0x93)], // reserved
            0x97 => 5, // SWA SetWindowAttributes: fill, border, scroll, display — all colour, positionskipped
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
        if self.windows[self.current].defined {
            self.style_pool(None);
            self.windows[self.current].text.push(ch);
        }
    }

    /// Ensure a style class exists for the given attributes.
    /// When `attrs` is None, read pen attributes from the current window.
    fn style_pool(&mut self, attrs: Option<(u8, u8)>) -> String {
        let class = match attrs {
            Some((a1, a2)) => {
                let italic = (a1 & 0x04) != 0;
                let underline = (a1 & 0x02) != 0;
                let font = a2 & 0x07;
                let sz = a1 >> 6;
                let mut parts = Vec::new();
                if italic {
                    parts.push("it".to_string());
                }
                if underline {
                    parts.push("ul".to_string());
                }
                if font != 0 {
                    parts.push(format!("f{}", font));
                }
                if sz != 0 {
                    parts.push(format!("s{}", sz));
                }
                if parts.is_empty() { String::new() } else { parts.join("_") }
            }
            None => String::new(),
        };
        let w = &self.windows[self.current];
        let color_class = if let Some((r, g, b)) = w.pen_fg {
            let name = format!("fg_{r}_{g}_{b}");
            if !self.styles.iter().any(|(k, _)| k == &name) {
                let css = format!(
                    "::cue(.{}) {{ color: rgb({}, {}, {}) }}",
                    name,
                    r * 85,
                    g * 85,
                    b * 85
                );
                self.styles.push((name.clone(), css));
            }
            name
        } else {
            String::new()
        };
        [&class[..], &color_class[..]]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    }
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

    fn dtvcc_frame1() -> Vec<u8> {
        // Frame 1: DF0(visible) + "Hi!" → 10-byte block, size_code=6, svc_hdr=0x2A
        // DF0: 0x98 + 6 args (0x20 visible, rest 0)
        // Text: Hi!
        // Packet: [hdr=0x06] [svc=0x2A] [DF0...] [Hi!] [pad to 12]
        let triples = [
            (3, 0x06, 0x2A),
            (2, 0x98, 0x20),
            (2, 0x00, 0x00),
            (2, 0x00, 0x00),
            (2, 0x00, 0x48),
            (2, 0x69, 0x21),
            (2, 0x00, 0x00), // Hi! + pad
        ];
        sei_au(&triples)
    }

    fn dtvcc_frame2() -> Vec<u8> {
        // Frame 2: DLW(0x01) + DF0(visible) + "CEA-708 verified."
        // Block: DLW(2) + DF0(7) + "CEA-708 verified."(17) = 26 bytes
        // svc_hdr = 0x20 | 26 = 0x3A, size_code = ceil(27/2) = 14, pkt_hdr = 0x0E
        let block = [
            0x8C, 0x01, // DLW window 0
            0x98, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, // DF0 visible
            b'C', b'E', b'A', b'-', b'7', b'0', b'8', b' ', b'v', b'e', b'r', b'i', b'f', b'i',
            b'e', b'd', b'.',
        ];
        let mut triples = vec![(3u8, 0x0E, 0x3A)];
        for i in (0..block.len()).step_by(2) {
            if i + 1 < block.len() {
                triples.push((2, block[i], block[i + 1]));
            } else {
                triples.push((2, block[i], 0x00));
            }
        }
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
    fn two_frame_dlw_cycle() {
        let f1 = dtvcc_frame1();
        let f2 = dtvcc_frame2();
        let samples = [(0u64, f1.as_slice()), (3600, f2.as_slice())];
        let triples = cc_triples(&samples, false);
        let tracks = decode(&triples);
        assert_eq!(tracks.len(), 1, "expected one 708 track");
        let t = &tracks[0];
        let _vtte_count =
            t.samples.iter().filter(|s| s.data.windows(4).any(|w| w == b"vtte")).count();
        let payl_count =
            t.samples.iter().filter(|s| s.data.windows(4).any(|w| w == b"payl")).count();
        assert!(payl_count >= 1, "expected >= 1 payl, got {payl_count}");
        let all_data: Vec<u8> = t.samples.iter().flat_map(|s| s.data.clone()).collect();
        assert!(all_data.windows(3).any(|w| w == b"Hi!"), "Hi! not found in decoded output");
        assert!(
            all_data.windows(8).any(|w| w == b"verified"),
            "verified not found in decoded output"
        );
    }

    #[test]
    fn dtvcc_build_packet_match() {
        // Verify that build_packet() used in the corpus generator produces
        // the same output as the manually-constructed dtvcc_frame2().
        use crate::sei::test_support::sei_au;

        // Replicate build_packet from generate_708_corpus_asset
        let build_packet = |frame: usize, text: &str| -> Vec<u8> {
            let mut block = Vec::new();
            if frame > 0 {
                block.extend_from_slice(&[0x8C, 0x01]);
            }
            block.extend_from_slice(&[0x98, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00]);
            block.extend_from_slice(text.as_bytes());
            let size_code = ((block.len() + 2) / 2).min(63) as u8;
            let svc_hdr = 0x20 | (block.len().min(31) as u8);
            let mut triples = vec![(3u8, size_code, svc_hdr)];
            for i in (0..block.len()).step_by(2) {
                if i + 1 < block.len() {
                    triples.push((2, block[i], block[i + 1]));
                } else {
                    triples.push((2, block[i], 0x00));
                }
            }
            sei_au(&triples)
        };
        let generated = build_packet(1, "CEA-708 verified.");
        let reference = dtvcc_frame2();
        assert_eq!(generated.len(), reference.len(), "length mismatch");
        let gen_ga94 = &generated[generated.iter().position(|&b| b == 0xb5).unwrap()..];
        let ref_ga94 = &reference[reference.iter().position(|&b| b == 0xb5).unwrap()..];
        assert_eq!(gen_ga94, ref_ga94, "GA94 payload mismatch");
    }

    /// Generate a synthetic H.264 Annex B clip with CEA-708 captions.
    ///
    /// Run with `cargo test -p sheathe-text -- generate_708_corpus_asset --ignored --nocapture`.
    #[test]
    #[ignore]
    fn generate_708_corpus_asset() {
        use crate::sei::test_support::sei_au;
        use std::fs;

        let captions = [
            "Hi!",
            "CEA-708 verified.",
            "DTVCC decoder works.",
            "Window model OK.",
            "Multiple windows.",
            "Timing matches.",
            "WebVTT output.",
            "Pop-on style.",
            "708 features.",
            "End-to-end test.",
        ];

        let corpus_file: std::path::PathBuf =
            [env!("CARGO_MANIFEST_DIR"), "..", "..", "corpus", "media", "bear-708.h264"]
                .iter()
                .collect();

        // Use known-good SPS (26 bytes) and PPS (5 bytes) from bear.h264
        let sps: &[u8] = &[
            0x67, 0x64, 0x00, 0x1f, 0xac, 0x34, 0xe5, 0x01, 0x40, 0x16, 0xec, 0x04, 0x40, 0x00,
            0x00, 0x19, 0x00, 0x00, 0x05, 0xda, 0xa3, 0xc6, 0x0c, 0x45, 0x80, 0x00,
        ];
        let pps: &[u8] = &[0x68, 0xee, 0xb2, 0xc8, 0xb0];
        // Slice: an IDR frame (type 5) from bear.h264
        let slice: &[u8] = &[
            0x65, 0x88, 0x80, 0x20, 0x01, 0xff, 0x98, 0x57, 0x23, 0x12, 0x68, 0x17, 0x2f, 0x66,
            0x04, 0x50, 0xf7, 0x05, 0x5f, 0x79, 0x8b, 0xd1, 0x9e, 0x7f, 0x5f, 0x20, 0x6d, 0x53,
            0xb5, 0x46,
        ];

        let build_packet = |frame: usize, text: &str| -> Vec<u8> {
            let mut block = Vec::new();
            if frame > 0 {
                block.extend_from_slice(&[0x8C, 0x01]);
            }
            block.extend_from_slice(&[0x98, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00]);
            block.extend_from_slice(text.as_bytes());
            let size_code = ((block.len() + 2) / 2).min(63) as u8;
            let svc_hdr = 0x20 | (block.len().min(31) as u8);
            let mut triples = vec![(3u8, size_code, svc_hdr)];
            for i in (0..block.len()).step_by(2) {
                if i + 1 < block.len() {
                    triples.push((2, block[i], block[i + 1]));
                } else {
                    triples.push((2, block[i], 0x00));
                }
            }
            sei_au(&triples)
        };

        let mut out = Vec::new();
        // SPS and PPS only once, before the first access unit
        for nal_data in [sps, pps] {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal_data);
        }
        for frame in 0..150 {
            // Slice first (VCL trigger for AU partitioner), then SEI.
            // This order ensures the partitioner assigns SEI to its own AU.
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&slice);
            let sei = build_packet(frame, captions[frame % captions.len()]);
            let sei_slice = sei.as_slice();
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(sei_slice);
        }

        fs::write(&corpus_file, &out).expect("write bear-708.h264");
        println!(
            "Wrote {} ({:.1} KB, 150 frames)",
            corpus_file.display(),
            out.len() as f64 / 1024.0
        );
    }

    #[test]
    fn no_dtvcc_no_tracks() {
        let au = sei_au(&[(0, 0x48, 0x49)]);
        let triples = cc_triples(&[(0, au.as_slice())], false);
        assert!(decode(&triples).is_empty());
    }

    #[test]
    fn service_block_split() {
        let pkt = [0x06, 0x29, 0x98, 0x20, 0, 0, 0, 0, 0, 0x48, 0x49, 0x00];
        let blocks = service_blocks(&pkt);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, 1);
    }
}

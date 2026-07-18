//! Minimal SCTE-35 splice_info_section builder for ad markers.
//!
//! Produces a well-formed binary `splice_info_section` carrying a
//! `splice_insert` command (cue-out / cue-in). Enough for packaging pipelines
//! to signal ad breaks via DASH `EventStream` and HLS `EXT-X-DATERANGE` without
//! a full SCTE-35 stack. The CRC-32 is computed per MPEG-2 Systems (ISO 13818-1
//! annex A).

/// A SCTE-35 cue placement relative to the presentation timeline.
#[derive(Debug, Clone)]
pub struct Scte35Marker {
    /// Presentation time in seconds from the period / asset start.
    pub time_seconds: f64,
    /// Cue-out (ad start) when true; cue-in (ad end / return-to-network) when false.
    pub out_of_network: bool,
    /// Optional event id (defaults to a hash of the time).
    pub event_id: Option<u32>,
    /// Optional break duration in seconds (cue-out only).
    pub break_duration_seconds: Option<f64>,
}

impl Scte35Marker {
    /// Cue-out (splice into ad) at `time_seconds`.
    pub fn cue_out(time_seconds: f64) -> Self {
        Self { time_seconds, out_of_network: true, event_id: None, break_duration_seconds: None }
    }

    /// Cue-in (return to network) at `time_seconds`.
    pub fn cue_in(time_seconds: f64) -> Self {
        Self { time_seconds, out_of_network: false, event_id: None, break_duration_seconds: None }
    }
}

/// Build a binary SCTE-35 `splice_info_section` for `marker`.
///
/// Uses `pts_adjustment = 0` and signals the splice time as PTS at 90 kHz
/// (`time_seconds * 90000`). Suitable for embedding as DASH Event message data
/// (base64) or HLS `SCTE35-OUT`/`SCTE35-IN` (hex with `0x` prefix).
pub fn build_splice_insert(marker: &Scte35Marker) -> Vec<u8> {
    let event_id = marker.event_id.unwrap_or_else(|| {
        // Stable-ish default from time (milliseconds).
        (marker.time_seconds * 1000.0).round() as u32
    });
    let pts = (marker.time_seconds * 90_000.0).round() as u64 & 0x1_ffff_ffff; // 33-bit PTS

    // splice_insert body (without section header / CRC).
    let mut body = Vec::new();
    // splice_event_id
    body.extend_from_slice(&event_id.to_be_bytes());
    // splice_event_cancel_indicator(1)=0 | reserved(7)=0x7f
    body.push(0x7f);
    // out_of_network_indicator(1) | program_splice_flag(1)=1 | duration_flag(1)
    // | splice_immediate_flag(1)=0 | reserved(4)=0xf
    let duration_flag = marker.break_duration_seconds.is_some() && marker.out_of_network;
    let mut flags: u8 = 0b0100_1111; // program_splice=1, immediate=0, reserved
    if marker.out_of_network {
        flags |= 0b1000_0000;
    }
    if duration_flag {
        flags |= 0b0010_0000;
    }
    body.push(flags);

    // program splice time: time_specified_flag(1)=1 | reserved(6)=0x3f | pts_time(33)
    // Encoded as 5 bytes: 1 bit flag + 6 reserved + 33 pts = 40 bits.
    let time_word: u64 = (1u64 << 39) | (0x3f << 33) | pts;
    body.push(((time_word >> 32) & 0xff) as u8);
    body.push(((time_word >> 24) & 0xff) as u8);
    body.push(((time_word >> 16) & 0xff) as u8);
    body.push(((time_word >> 8) & 0xff) as u8);
    body.push((time_word & 0xff) as u8);

    if duration_flag {
        // break_duration: auto_return(1)=1 | reserved(6)=0x3f | duration(33) @ 90kHz
        let dur_pts = (marker.break_duration_seconds.unwrap_or(0.0) * 90_000.0).round() as u64
            & 0x1_ffff_ffff;
        let dur_word: u64 = (1u64 << 39) | (0x3f << 33) | dur_pts;
        body.push(((dur_word >> 32) & 0xff) as u8);
        body.push(((dur_word >> 24) & 0xff) as u8);
        body.push(((dur_word >> 16) & 0xff) as u8);
        body.push(((dur_word >> 8) & 0xff) as u8);
        body.push((dur_word & 0xff) as u8);
    }

    // unique_program_id (16), avail_num (8), avails_expected (8)
    body.extend_from_slice(&0u16.to_be_bytes());
    body.push(0);
    body.push(0);

    // Section: table_id(8)=0xFC | section_syntax_indicator(1)=0 | private(1)=0
    // | reserved(2)=3 | section_length(12) | protocol_version(8)=0
    // | encrypted_packet(1)=0 | encryption_algorithm(6)=0 | pts_adjustment(33)=0
    // | cw_index(8)=0 | tier(12)=0xfff | splice_command_length(12)
    // | splice_command_type(8)=5 (splice_insert) | command | descriptor_loop_length(16)=0
    // | CRC_32(32)

    let command_type: u8 = 5; // splice_insert
    let command_len = body.len() as u16;
    // Bytes after section_length field, before CRC:
    // protocol(1) + encrypt/pts_adj(5) + cw(1) + tier/cmd_len(3) + cmd_type(1)
    // + body + descriptor_loop(2)
    let section_payload_len = 1 + 5 + 1 + 3 + 1 + body.len() + 2;
    let section_length = (section_payload_len + 4) as u16; // + CRC

    let mut section = Vec::with_capacity(3 + section_payload_len + 4);
    section.push(0xFC); // table_id
    // section_syntax=0, private=0, reserved=3, section_length
    let sl = (0b0011_0000_0000_0000) | (section_length & 0x0fff);
    section.extend_from_slice(&sl.to_be_bytes());
    section.push(0); // protocol_version
    // encrypted_packet=0 | encryption_algorithm=0 | pts_adjustment (33 bits of 0)
    // 40 bits: 1+6+33 packed into 5 bytes starting with reserved high bits.
    section.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00]);
    section.push(0); // cw_index
    // tier (12 bits of 1) | splice_command_length (12 bits)
    let tier_cmd = (0x0fffu32 << 12) | (u32::from(command_len) & 0x0fff);
    section.push(((tier_cmd >> 16) & 0xff) as u8);
    section.push(((tier_cmd >> 8) & 0xff) as u8);
    section.push((tier_cmd & 0xff) as u8);
    section.push(command_type);
    section.extend_from_slice(&body);
    section.extend_from_slice(&0u16.to_be_bytes()); // descriptor_loop_length

    let crc = mpeg_crc32(&section);
    section.extend_from_slice(&crc.to_be_bytes());
    section
}

/// Hex encoding with a `0x` prefix (HLS `SCTE35-OUT` style).
pub fn to_hex_0x(bytes: &[u8]) -> String {
    let mut s = String::from("0x");
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Standard base64 (no padding strip) for DASH Event message data.
pub fn to_base64(bytes: &[u8]) -> String {
    // Minimal base64 encoder — avoids a dependency for a single encode site.
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n =
            (u32::from(bytes[i]) << 16) | (u32::from(bytes[i + 1]) << 8) | u32::from(bytes[i + 2]);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    let rest = bytes.len() - i;
    if rest == 1 {
        let n = u32::from(bytes[i]) << 16;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let n = (u32::from(bytes[i]) << 16) | (u32::from(bytes[i + 1]) << 8);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

/// MPEG-2 CRC-32 (ISO/IEC 13818-1 annex A), poly 0x04C11DB7, init 0xFFFFFFFF.
fn mpeg_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            if crc & 0x8000_0000 != 0 {
                crc = (crc << 1) ^ 0x04C1_1DB7;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_insert_has_table_id_and_crc_length() {
        let bytes = build_splice_insert(&Scte35Marker::cue_out(12.0));
        assert_eq!(bytes[0], 0xFC);
        // section_length covers remaining bytes after the 3-byte header.
        let section_length = u16::from_be_bytes([bytes[1] & 0x0f, bytes[2]]) as usize;
        assert_eq!(bytes.len(), 3 + section_length);
        // Command type splice_insert = 5 appears after the fixed header fields.
        assert!(bytes.contains(&5));
    }

    #[test]
    fn hex_and_base64_round_shapes() {
        let bytes = build_splice_insert(&Scte35Marker::cue_in(30.0));
        let hex = to_hex_0x(&bytes);
        assert!(hex.starts_with("0xFC"));
        let b64 = to_base64(&bytes);
        assert!(!b64.is_empty());
        assert!(b64.is_ascii());
    }
}

//! MPEG-1/2 Audio Layer III (MP3) frame parsing.

use sheathe_core::{Sample, SampleFlags};

/// Parsed MP3 frame header.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mp3Header {
    /// Decoded sampling rate in Hz.
    pub sample_rate: u32,
    /// Channel count (1 for mono, else 2).
    pub channels: u8,
    /// Coded PCM samples per frame (1152 for MPEG-1, 576 for MPEG-2/2.5).
    pub samples_per_frame: u32,
    /// Frame length in bytes.
    pub frame_size: usize,
    /// MPEG-2/2.5 audio (OTI `0x69`) vs MPEG-1 (OTI `0x6B`).
    pub is_mpeg2: bool,
}

// Bitrate (kbit/s) by index — Layer III only. Index 0 = "free", 15 = invalid.
const BITRATE_V1_L3: [u32; 16] =
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const BITRATE_V2_L3: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];

const SR_V1: [u32; 3] = [44_100, 48_000, 32_000]; // MPEG-1
const SR_V2: [u32; 3] = [22_050, 24_000, 16_000]; // MPEG-2
const SR_V25: [u32; 3] = [11_025, 12_000, 8_000]; // MPEG-2.5

/// Parse an MP3 frame header at `data[0..4]`. Only Layer III is accepted.
pub(crate) fn parse_header(data: &[u8]) -> Option<Mp3Header> {
    if data.len() < 4 || data[0] != 0xff || (data[1] & 0xe0) != 0xe0 {
        return None;
    }
    let version = (data[1] >> 3) & 0x03; // 0=2.5, 2=2, 3=1 (1=reserved)
    let layer = (data[1] >> 1) & 0x03; // 1 = Layer III
    if version == 1 || layer != 0x01 {
        return None;
    }
    let bitrate_idx = (data[2] >> 4) & 0x0f;
    let sr_idx = (data[2] >> 2) & 0x03;
    let padding = u32::from((data[2] >> 1) & 0x01);
    let channel_mode = (data[3] >> 6) & 0x03;
    if bitrate_idx == 0 || bitrate_idx == 15 || sr_idx == 3 {
        return None; // free-format / invalid not supported
    }

    let is_mpeg1 = version == 3;
    let bitrate = if is_mpeg1 { BITRATE_V1_L3 } else { BITRATE_V2_L3 }[usize::from(bitrate_idx)];
    let sample_rate = match version {
        3 => SR_V1,
        2 => SR_V2,
        _ => SR_V25,
    }[usize::from(sr_idx)];
    let bitrate_bps = bitrate * 1000;

    // Layer III: 1152 samples/frame (MPEG-1) or 576 (MPEG-2/2.5).
    let (samples_per_frame, coeff) = if is_mpeg1 { (1152u32, 144u32) } else { (576u32, 72u32) };
    let frame_size = ((coeff * bitrate_bps / sample_rate) + padding) as usize;
    if frame_size < 4 {
        return None;
    }
    let channels = if channel_mode == 0x03 { 1 } else { 2 };

    Some(Mp3Header { sample_rate, channels, samples_per_frame, frame_size, is_mpeg2: !is_mpeg1 })
}

/// Read the header of the first MP3 frame, skipping an optional ID3v2 tag and
/// scanning for the frame sync.
pub(crate) fn first_header(data: &[u8]) -> Option<Mp3Header> {
    let start = id3v2_len(data);
    let mut off = start;
    while off + 4 <= data.len() {
        if data[off] == 0xff && (data[off + 1] & 0xe0) == 0xe0 {
            if let Some(h) = parse_header(&data[off..]) {
                return Some(h);
            }
        }
        off += 1;
    }
    None
}

/// Split an MP3 byte stream into per-frame samples in decode order.
pub(crate) fn frames(data: &[u8]) -> Vec<Sample> {
    let mut samples = Vec::new();
    let mut off = id3v2_len(data);
    let mut ts = 0u64;
    while off + 4 <= data.len() {
        if data[off] != 0xff || (data[off + 1] & 0xe0) != 0xe0 {
            off += 1;
            continue;
        }
        let Some(h) = parse_header(&data[off..]) else {
            off += 1;
            continue;
        };
        let end = off + h.frame_size;
        if end > data.len() {
            break;
        }
        let ticks = ((u64::from(h.samples_per_frame) * 90_000) / u64::from(h.sample_rate.max(1)))
            .max(1) as u32;
        samples.push(Sample {
            dts: ts,
            pts: ts,
            duration: ticks,
            flags: SampleFlags::KEYFRAME,
            data: data[off..end].to_vec(),
        });
        ts += u64::from(ticks);
        off = end;
    }
    samples
}

/// Length of a leading ID3v2 tag (`ID3` + syncsafe size), or 0 if absent.
fn id3v2_len(data: &[u8]) -> usize {
    if data.len() < 10 || &data[0..3] != b"ID3" {
        return 0;
    }
    // 4 syncsafe bytes (7 bits each) → tag size, plus the 10-byte header.
    let size = (usize::from(data[6]) << 21)
        | (usize::from(data[7]) << 14)
        | (usize::from(data[8]) << 7)
        | usize::from(data[9]);
    10 + size
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MPEG-1 Layer III, 128 kbps, 44.1 kHz, stereo → 417-byte frame (+pad off).
    fn frame() -> Vec<u8> {
        // 0xFF 0xFB: sync + MPEG1 + Layer III + no CRC.
        // 0x90: bitrate_idx 9 (128k), sr_idx 0 (44.1k), no padding.
        // 0x00: stereo.
        let mut f = vec![0xff, 0xfb, 0x90, 0x00];
        let h = parse_header(&f).unwrap();
        f.resize(h.frame_size, 0);
        f
    }

    #[test]
    fn parses_mp3_header() {
        let h = parse_header(&[0xff, 0xfb, 0x90, 0x00]).expect("header");
        assert_eq!(h.sample_rate, 44_100);
        assert_eq!(h.channels, 2);
        assert_eq!(h.samples_per_frame, 1152);
        assert_eq!(h.frame_size, 417); // 144*128000/44100 = 417
        assert!(!h.is_mpeg2);
    }

    #[test]
    fn splits_frames_and_skips_id3() {
        let mut data = vec![b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 3]; // 3-byte ID3 body
        data.extend_from_slice(&[0, 0, 0]);
        data.extend_from_slice(&frame());
        data.extend_from_slice(&frame());
        let s = frames(&data);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].duration, 2_351); // 1152/44100 @ 90k
    }
}

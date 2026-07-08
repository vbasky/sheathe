//! Native FLAC (`fLaC`) stream parsing: STREAMINFO + audio-frame splitting.

use crate::bitstream::BitReader;
use sheathe_core::{Sample, SampleFlags};

/// The FLAC stream marker.
pub(crate) const MAGIC: &[u8; 4] = b"fLaC";

/// Fields decoded from the STREAMINFO metadata block.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamInfo {
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    /// Maximum block size in samples (per-frame sample count for fixed streams).
    pub max_block_size: u32,
}

/// Locate the STREAMINFO metadata block and return `(parsed, raw 34 bytes)`.
pub(crate) fn stream_info(data: &[u8]) -> Option<(StreamInfo, &[u8])> {
    if data.len() < 8 || &data[0..4] != MAGIC {
        return None;
    }
    // First metadata block header (STREAMINFO, type 0) starts right after `fLaC`.
    let block_type = data[4] & 0x7f;
    let len = (usize::from(data[5]) << 16) | (usize::from(data[6]) << 8) | usize::from(data[7]);
    if block_type != 0 || len < 34 || 8 + 34 > data.len() {
        return None;
    }
    let si = &data[8..8 + 34];
    let mut br = BitReader::new(si);
    let _min_bs = br.read_u(16)?;
    let max_block_size = br.read_u(16)?;
    let _min_fs = br.read_u(24)?;
    let _max_fs = br.read_u(24)?;
    let sample_rate = br.read_u(20)?;
    let channels = br.read_u(3)? as u8 + 1;
    let bits_per_sample = br.read_u(5)? as u8 + 1;
    Some((StreamInfo { sample_rate, channels, bits_per_sample, max_block_size }, si))
}

/// Byte offset where audio frames begin (after all metadata blocks).
fn audio_start(data: &[u8]) -> Option<usize> {
    if data.len() < 4 || &data[0..4] != MAGIC {
        return None;
    }
    let mut off = 4;
    loop {
        if off + 4 > data.len() {
            return None;
        }
        let last = data[off] & 0x80 != 0;
        let len = (usize::from(data[off + 1]) << 16)
            | (usize::from(data[off + 2]) << 8)
            | usize::from(data[off + 3]);
        off += 4 + len;
        if last {
            return Some(off);
        }
    }
}

/// True where a FLAC audio-frame sync code (14 bits, `0b11111111111110`) begins.
fn is_frame_sync(data: &[u8], off: usize) -> bool {
    off + 1 < data.len() && data[off] == 0xff && (data[off + 1] & 0xfc) == 0xf8
}

/// Split a native FLAC stream into per-frame samples.
///
/// Frame boundaries are found by scanning for the next sync code (a pragmatic
/// splitter for fixed-block-size streams); durations use the STREAMINFO block
/// size. Robust variable-block-size handling is left for a later pass.
pub(crate) fn frames(data: &[u8]) -> Vec<Sample> {
    let Some((si, _)) = stream_info(data) else { return Vec::new() };
    let Some(start) = audio_start(data) else { return Vec::new() };
    let ticks =
        ((u64::from(si.max_block_size) * 90_000) / u64::from(si.sample_rate.max(1))).max(1) as u32;

    // Collect frame start offsets.
    let mut starts = Vec::new();
    let mut off = start;
    while off + 2 <= data.len() {
        if is_frame_sync(data, off) {
            starts.push(off);
            off += 2; // step past this sync before hunting the next
        } else {
            off += 1;
        }
    }

    let mut samples = Vec::new();
    for (i, &s) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(data.len());
        let ts = i as u64 * u64::from(ticks);
        samples.push(Sample {
            dts: ts,
            pts: ts,
            duration: ticks,
            flags: SampleFlags::KEYFRAME,
            data: data[s..end].to_vec(),
        });
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal native FLAC stream: `fLaC` + STREAMINFO + two frames.
    fn flac_stream() -> Vec<u8> {
        let mut si = [0u8; 34];
        // min/max block size = 4096; min/max frame size = 0.
        si[0..2].copy_from_slice(&4096u16.to_be_bytes());
        si[2..4].copy_from_slice(&4096u16.to_be_bytes());
        // sample_rate(20)=44100, channels-1(3)=1 (→2ch), bps-1(5)=15 (→16).
        // 44100 = 0xAC44 → 20 bits: 0000_1010_1100_0100_0100
        // Pack bytes 10..13: sr[19:12], sr[11:4], sr[3:0]<<4 | (ch-1)<<1 | bps5>>4 ...
        // Easier: write via bit layout.
        // bits: sr(20) ch(3)=1 bps(5)=15 then total_samples(36) md5(128)
        let sr: u32 = 44_100;
        let ch_minus1: u32 = 1;
        let bps_minus1: u32 = 15;
        let packed: u64 =
            (u64::from(sr) << 44) | (u64::from(ch_minus1) << 41) | (u64::from(bps_minus1) << 36);
        si[10..18].copy_from_slice(&packed.to_be_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(0x80); // last-metadata-block=1, type=0 (STREAMINFO)
        out.extend_from_slice(&[0x00, 0x00, 0x22]); // length = 34
        out.extend_from_slice(&si);
        // Two audio frames, each starting with a sync code.
        for _ in 0..2 {
            out.extend_from_slice(&[0xff, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        }
        out
    }

    #[test]
    fn parses_streaminfo() {
        let stream = flac_stream();
        let (si, raw) = stream_info(&stream).expect("streaminfo");
        assert_eq!(si.sample_rate, 44_100);
        assert_eq!(si.channels, 2);
        assert_eq!(si.bits_per_sample, 16);
        assert_eq!(si.max_block_size, 4096);
        assert_eq!(raw.len(), 34);
    }

    #[test]
    fn splits_frames() {
        let s = frames(&flac_stream());
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].duration, 8_359); // 4096/44100 @ 90k
    }
}

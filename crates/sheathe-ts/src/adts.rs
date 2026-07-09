//! ADTS AAC frame parsing.

use sheathe_core::{Sample, SampleFlags};

/// Split an ADTS byte stream into AAC access units.
pub(crate) fn frames(data: &[u8], pts: u64, dts: u64, sample_rate: u32) -> Vec<Sample> {
    // 1024 PCM samples per AAC frame; guard the divisor against sub-1024 rates.
    let frames_per_sec = (sample_rate / 1024).max(1);
    let ticks_per_frame = (90_000u64 / u64::from(frames_per_sec)).max(1) as u32;
    let mut samples = Vec::new();
    let mut off = 0;
    let mut index = 0u64;
    while off + 7 <= data.len() {
        if data[off] != 0xff || (data[off + 1] & 0xf0) != 0xf0 {
            off += 1;
            continue;
        }
        let frame_len = u16::from(data[off + 3] & 0x03) << 11
            | u16::from(data[off + 4]) << 3
            | u16::from(data[off + 5] >> 5);
        let frame_len = usize::from(frame_len);
        if frame_len < 7 || off + frame_len > data.len() {
            off += 1;
            continue;
        }
        samples.push(Sample {
            dts: dts.saturating_add(index * u64::from(ticks_per_frame)),
            pts: pts.saturating_add(index * u64::from(ticks_per_frame)),
            duration: ticks_per_frame,
            flags: SampleFlags::KEYFRAME, // every AAC frame is independently decodable
            data: data[off..off + frame_len].to_vec(),
        });
        index += 1;
        off += frame_len;
    }
    samples
}

/// Offset of the first ADTS syncword (`0xFFF`) in `data`, if any.
pub(crate) fn find_sync(data: &[u8]) -> Option<usize> {
    (0..data.len().saturating_sub(6)).find(|&i| data[i] == 0xff && (data[i + 1] & 0xf0) == 0xf0)
}

/// Read the AAC sample rate from the first ADTS header, if present.
///
/// Scans for the syncword: real transport streams do not place the ADTS frame
/// exactly at the start of the reassembled PES payload.
pub(crate) fn sample_rate_hz(data: &[u8]) -> Option<u32> {
    let off = find_sync(data)?;
    let idx = (data[off + 2] & 0x3c) >> 2;
    ADTS_SAMPLE_RATES.get(usize::from(idx)).copied().filter(|&r| r != 0)
}

/// ADTS sampling-frequency-index → Hz (ISO/IEC 13818-7 Table 35).
pub(crate) const ADTS_SAMPLE_RATES: [u32; 16] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350, 0, 0, 0,
];

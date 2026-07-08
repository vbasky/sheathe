//! AC-3 (ATSC A/52 / ETSI TS 102 366) syncframe parsing.
//!
//! Splits a raw AC-3 elementary stream into per-syncframe [`Sample`]s and reads
//! the bit-stream information (BSI) header needed to synthesise a `dac3` box.

use crate::bitstream::BitReader;
use sheathe_core::{Sample, SampleFlags};

/// AC-3 syncword (`0x0B77`), big-endian, at the start of every syncframe.
pub(crate) const SYNCWORD: [u8; 2] = [0x0b, 0x77];

/// Coded audio samples carried by one AC-3 syncframe (fixed by the standard).
pub(crate) const SAMPLES_PER_FRAME: u32 = 1536;

/// Parsed AC-3 bit-stream information for one syncframe.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ac3Header {
    /// Sample-rate code (0=48k, 1=44.1k, 2=32k).
    pub fscod: u8,
    /// Bit-stream identification (≤ 8 for AC-3; 16 marks E-AC-3).
    pub bsid: u8,
    /// Bit-stream mode (main/associated service type).
    pub bsmod: u8,
    /// Audio coding mode (channel layout, excluding LFE).
    pub acmod: u8,
    /// Low-frequency-effects channel present.
    pub lfeon: bool,
    /// Nominal bit-rate code (`frmsizecod >> 1`), stored verbatim in `dac3`.
    pub bit_rate_code: u8,
    /// Decoded sampling rate in Hz.
    pub sample_rate: u32,
    /// Full channel count including LFE.
    pub channels: u8,
    /// Total syncframe length in bytes.
    pub frame_size: usize,
}

/// `acmod` → number of full-bandwidth channels (A/52 Table 5.8), before LFE.
const ACMOD_CHANNELS: [u8; 8] = [2, 1, 2, 3, 3, 4, 4, 5];

/// Sampling rate in Hz indexed by `fscod` (A/52 §5.4.1.3); index 3 is reserved.
const FSCOD_RATES: [u32; 3] = [48_000, 44_100, 32_000];

/// Syncframe size in 16-bit words indexed by `frmsizecod` (0..38), one column
/// per `fscod` (48 kHz, 44.1 kHz, 32 kHz) — A/52 Table 5.18. The 44.1 kHz
/// column carries the padding word encoded by the low bit of `frmsizecod`.
const FRAME_SIZE_WORDS: [[u16; 3]; 38] = [
    [64, 69, 96],
    [64, 70, 96],
    [80, 87, 120],
    [80, 88, 120],
    [96, 104, 144],
    [96, 105, 144],
    [112, 121, 168],
    [112, 122, 168],
    [128, 139, 192],
    [128, 140, 192],
    [160, 174, 240],
    [160, 175, 240],
    [192, 208, 288],
    [192, 209, 288],
    [224, 243, 336],
    [224, 244, 336],
    [256, 278, 384],
    [256, 279, 384],
    [320, 348, 480],
    [320, 349, 480],
    [384, 417, 576],
    [384, 418, 576],
    [448, 487, 672],
    [448, 488, 672],
    [512, 557, 768],
    [512, 558, 768],
    [640, 696, 960],
    [640, 697, 960],
    [768, 835, 1152],
    [768, 836, 1152],
    [896, 975, 1344],
    [896, 976, 1344],
    [1024, 1114, 1536],
    [1024, 1115, 1536],
    [1152, 1253, 1728],
    [1152, 1254, 1728],
    [1280, 1393, 1920],
    [1280, 1394, 1920],
];

/// Parse the BSI header of the syncframe beginning at `data[0]`.
///
/// Returns `None` when the syncword is absent, the codes are reserved, or the
/// stream is E-AC-3 (`bsid == 16`), which uses a different frame structure.
pub(crate) fn parse_header(data: &[u8]) -> Option<Ac3Header> {
    if data.len() < 6 || data[0..2] != SYNCWORD {
        return None;
    }
    // Skip syncword (16) + crc1 (16); BSI begins at byte 4.
    let mut br = BitReader::new(&data[4..]);
    let fscod = br.read_u(2)? as u8;
    let frmsizecod = br.read_u(6)? as u8;
    let bsid = br.read_u(5)? as u8;
    let bsmod = br.read_u(3)? as u8;
    let acmod = br.read_u(3)? as u8;

    // AC-3 only: E-AC-3 (bsid 16) and the alt-rate variants (9/10) are out of scope.
    if bsid > 8 || fscod > 2 || frmsizecod as usize >= FRAME_SIZE_WORDS.len() {
        return None;
    }

    // Optional mix-level / surround fields precede `lfeon` (A/52 §5.3).
    if acmod & 0x01 != 0 && acmod != 0x01 {
        br.read_u(2)?; // cmixlev
    }
    if acmod & 0x04 != 0 {
        br.read_u(2)?; // surmixlev
    }
    if acmod == 0x02 {
        br.read_u(2)?; // dsurmod
    }
    let lfeon = br.read_u1()? != 0;

    let sample_rate = FSCOD_RATES[usize::from(fscod)];
    let frame_size = usize::from(FRAME_SIZE_WORDS[usize::from(frmsizecod)][usize::from(fscod)]) * 2;
    if frame_size == 0 || frame_size > data.len() {
        return None;
    }
    let channels = ACMOD_CHANNELS[usize::from(acmod)] + u8::from(lfeon);

    Some(Ac3Header {
        fscod,
        bsid,
        bsmod,
        acmod,
        lfeon,
        bit_rate_code: frmsizecod >> 1,
        sample_rate,
        channels,
        frame_size,
    })
}

/// Read the header of the first syncframe in `data`, scanning for the syncword.
pub(crate) fn first_header(data: &[u8]) -> Option<Ac3Header> {
    let mut off = 0;
    while off + 6 <= data.len() {
        if data[off..off + 2] == SYNCWORD {
            if let Some(h) = parse_header(&data[off..]) {
                return Some(h);
            }
        }
        off += 1;
    }
    None
}

/// Split an AC-3 byte stream into per-syncframe samples in decode order.
pub(crate) fn frames(data: &[u8], sample_rate: u32) -> Vec<Sample> {
    // 1536 audio samples per frame; scale to the 90 kHz MPEG-TS timescale.
    let ticks_per_frame =
        ((u64::from(SAMPLES_PER_FRAME) * 90_000) / u64::from(sample_rate.max(1))).max(1) as u32;
    let mut samples = Vec::new();
    let mut off = 0;
    let mut index = 0u64;
    while off + 6 <= data.len() {
        if data[off..off + 2] != SYNCWORD {
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
        let ts = index * u64::from(ticks_per_frame);
        samples.push(Sample {
            dts: ts,
            pts: ts,
            duration: ticks_per_frame,
            // Every AC-3 syncframe is independently decodable.
            flags: SampleFlags::KEYFRAME,
            data: data[off..end].to_vec(),
        });
        index += 1;
        off = end;
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 48 kHz, frmsizecod 0 (64 words = 128 bytes), bsid 8, acmod 2 (2/0), no LFE.
    fn frame() -> Vec<u8> {
        let mut f = vec![0x0b, 0x77, 0x00, 0x00, 0x00, 0x40, 0x40];
        f.resize(128, 0);
        f
    }

    #[test]
    fn parses_bsi_header() {
        let h = parse_header(&frame()).expect("header");
        assert_eq!(h.fscod, 0);
        assert_eq!(h.sample_rate, 48_000);
        assert_eq!(h.bsid, 8);
        assert_eq!(h.acmod, 2);
        assert_eq!(h.channels, 2);
        assert!(!h.lfeon);
        assert_eq!(h.bit_rate_code, 0);
        assert_eq!(h.frame_size, 128);
    }

    #[test]
    fn rejects_non_syncword() {
        assert!(parse_header(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x40]).is_none());
    }

    #[test]
    fn splits_multiple_frames() {
        let mut data = frame();
        data.extend_from_slice(&frame());
        let samples = frames(&data, 48_000);
        assert_eq!(samples.len(), 2);
        // 1536 samples / 48 kHz → 2880 ticks @ 90 kHz.
        assert_eq!(samples[0].duration, 2_880);
        assert_eq!(samples[1].dts, 2_880);
    }

    #[test]
    fn first_header_scans_for_syncword() {
        let mut data = vec![0xff, 0xff, 0xff]; // leading garbage
        data.extend_from_slice(&frame());
        assert_eq!(first_header(&data).expect("header").sample_rate, 48_000);
    }
}

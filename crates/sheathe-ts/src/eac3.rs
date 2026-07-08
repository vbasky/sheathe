//! Enhanced AC-3 / E-AC-3 (ETSI TS 102 366 Annex E) syncframe parsing.
//!
//! Shares the `0x0B77` syncword with AC-3 but uses a different frame layout
//! (`bsid == 16`). Splits a raw E-AC-3 stream into per-syncframe [`Sample`]s and
//! reads the header needed to synthesise a `dec3` box.

use crate::ac3::SYNCWORD;
use crate::bitstream::BitReader;
use sheathe_core::{Sample, SampleFlags};

/// Parsed E-AC-3 bit-stream information for one syncframe.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Eac3Header {
    /// Stream type (0 = independent, 1 = dependent, 2 = AC-3-compatible independent).
    pub strmtyp: u8,
    /// Sample-rate code (0=48k, 1=44.1k, 2=32k; 3 selects `fscod2`).
    pub fscod: u8,
    /// Bit-stream identification (16 for E-AC-3).
    pub bsid: u8,
    /// Audio coding mode (channel layout, excluding LFE).
    pub acmod: u8,
    /// Low-frequency-effects channel present.
    pub lfeon: bool,
    /// Decoded sampling rate in Hz.
    pub sample_rate: u32,
    /// Full channel count including LFE.
    pub channels: u8,
    /// Coded audio samples in this frame (`numblocks * 256`).
    pub samples_per_frame: u32,
    /// Total syncframe length in bytes.
    pub frame_size: usize,
}

/// `acmod` → number of full-bandwidth channels (A/52 Table 5.8), before LFE.
const ACMOD_CHANNELS: [u8; 8] = [2, 1, 2, 3, 3, 4, 4, 5];
/// Sampling rate in Hz indexed by `fscod` (fscod 3 defers to `fscod2`).
const FSCOD_RATES: [u32; 3] = [48_000, 44_100, 32_000];
/// Reduced sampling rate in Hz indexed by `fscod2` (used when `fscod == 3`).
const FSCOD2_RATES: [u32; 3] = [24_000, 22_050, 16_000];
/// Number of audio blocks per syncframe indexed by `numblkscod`.
const NUMBLKS: [u32; 4] = [1, 2, 3, 6];

/// Parse the BSI header of the E-AC-3 syncframe beginning at `data[0]`.
///
/// Note: `bsid` sits at bit offset 40 (byte 5, top 5 bits) — the same position
/// in AC-3 and E-AC-3 — which is how callers disambiguate the two (16 ⇒ E-AC-3).
pub(crate) fn parse_header(data: &[u8]) -> Option<Eac3Header> {
    if data.len() < 6 || data[0..2] != SYNCWORD {
        return None;
    }
    // BSI begins right after the syncword (no leading crc in E-AC-3).
    let mut br = BitReader::new(&data[2..]);
    let strmtyp = br.read_u(2)? as u8;
    let _substreamid = br.read_u(3)?;
    let frmsiz = br.read_u(11)?;
    let fscod = br.read_u(2)? as u8;

    let (sample_rate, numblocks) = if fscod == 3 {
        let fscod2 = br.read_u(2)? as u8;
        if fscod2 > 2 {
            return None;
        }
        (FSCOD2_RATES[usize::from(fscod2)], 6) // numblkscod implied = 6 blocks
    } else {
        let numblkscod = br.read_u(2)? as usize;
        (FSCOD_RATES[usize::from(fscod)], NUMBLKS[numblkscod])
    };

    let acmod = br.read_u(3)? as u8;
    let lfeon = br.read_u1()? != 0;
    let bsid = br.read_u(5)? as u8;
    if bsid != 16 {
        return None; // not E-AC-3
    }

    let frame_size = (usize::try_from(frmsiz).ok()? + 1) * 2;
    if frame_size == 0 || frame_size > data.len() {
        return None;
    }
    let channels = ACMOD_CHANNELS[usize::from(acmod)] + u8::from(lfeon);

    Some(Eac3Header {
        strmtyp,
        fscod,
        bsid,
        acmod,
        lfeon,
        sample_rate,
        channels,
        samples_per_frame: numblocks * 256,
        frame_size,
    })
}

/// Read the header of the first E-AC-3 syncframe in `data`, scanning for the syncword.
pub(crate) fn first_header(data: &[u8]) -> Option<Eac3Header> {
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

/// Split an E-AC-3 byte stream into per-syncframe samples in decode order.
///
/// Dependent substreams (`strmtyp == 1`) are appended to the preceding
/// independent frame so each emitted sample is a complete coded audio frame.
pub(crate) fn frames(data: &[u8]) -> Vec<Sample> {
    let mut samples: Vec<Sample> = Vec::new();
    let mut off = 0;
    let mut ts = 0u64;
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
        if h.strmtyp == 1 {
            // Dependent substream: fold into the previous independent frame.
            if let Some(prev) = samples.last_mut() {
                prev.data.extend_from_slice(&data[off..end]);
                off = end;
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One independent E-AC-3 syncframe: 48 kHz, 6 blocks, acmod 7 (3/2), +LFE (5.1),
    /// frmsiz encodes a 96-byte frame.
    fn frame() -> Vec<u8> {
        // syncword | strmtyp(2)=0 substreamid(3)=0 frmsiz(11)=47 | fscod(2)=0
        // numblkscod(2)=3 acmod(3)=7 lfeon(1)=1 bsid(5)=16 ...
        // Bytes 2..: 00000 00000101111 00 11 111 1 10000 ...
        // Pack the first 6 bytes precisely (see test asserts).
        let mut br = BitVec::new();
        br.push(0, 2); // strmtyp
        br.push(0, 3); // substreamid
        br.push(47, 11); // frmsiz -> (47+1)*2 = 96 bytes
        br.push(0, 2); // fscod = 0 (48k)
        br.push(3, 2); // numblkscod = 3 (6 blocks)
        br.push(7, 3); // acmod = 7 (3/2)
        br.push(1, 1); // lfeon
        br.push(16, 5); // bsid = 16
        let mut f = vec![0x0b, 0x77];
        f.extend_from_slice(&br.into_bytes());
        f.resize(96, 0);
        f
    }

    /// Minimal MSB-first bit packer for building test frames.
    struct BitVec {
        bits: Vec<bool>,
    }
    impl BitVec {
        fn new() -> Self {
            Self { bits: Vec::new() }
        }
        fn push(&mut self, v: u32, n: u8) {
            for i in (0..n).rev() {
                self.bits.push((v >> i) & 1 == 1);
            }
        }
        fn into_bytes(self) -> Vec<u8> {
            let mut out = vec![0u8; self.bits.len().div_ceil(8)];
            for (i, b) in self.bits.iter().enumerate() {
                if *b {
                    out[i / 8] |= 1 << (7 - (i % 8));
                }
            }
            out
        }
    }

    #[test]
    fn parses_eac3_header() {
        let h = parse_header(&frame()).expect("header");
        assert_eq!(h.bsid, 16);
        assert_eq!(h.fscod, 0);
        assert_eq!(h.sample_rate, 48_000);
        assert_eq!(h.acmod, 7);
        assert_eq!(h.channels, 6); // 3/2 + LFE = 5.1
        assert!(h.lfeon);
        assert_eq!(h.samples_per_frame, 1536);
        assert_eq!(h.frame_size, 96);
    }

    #[test]
    fn splits_frames() {
        let mut data = frame();
        data.extend_from_slice(&frame());
        let s = frames(&data);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].duration, 2_880); // 1536/48k @ 90k
        assert_eq!(s[1].dts, 2_880);
    }
}

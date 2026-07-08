//! Build an `ec-3` AudioSampleEntry (with its `dec3` box) from an E-AC-3 header.

use crate::eac3::Eac3Header;

/// Build a complete `ec-3` AudioSampleEntry box (header included) from the first
/// E-AC-3 syncframe. Returns `None` when the syncframe cannot be parsed.
pub(crate) fn eac3_sample_entry(frame: &[u8]) -> Option<Vec<u8>> {
    let h = crate::eac3::parse_header(frame)?;
    let dec3 = dec3_box(&h);

    let mut out = Vec::new();
    let body_len = 28 + dec3.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(b"ec-3");
    out.extend_from_slice(&[0; 6]); // reserved (SampleEntry)
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 8]); // reserved (AudioSampleEntry)
    out.extend_from_slice(&u16::from(h.channels).to_be_bytes()); // channelcount
    out.extend_from_slice(&16u16.to_be_bytes()); // samplesize
    out.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&(h.sample_rate << 16).to_be_bytes()); // samplerate (16.16)
    out.extend_from_slice(&dec3);
    Some(out)
}

/// RFC 6381 codec string for E-AC-3 (no configuration suffix).
pub(crate) fn eac3_codec_string() -> String {
    "ec-3".to_string()
}

/// A minimal MSB-first bit accumulator for packing config-box payloads.
struct BitWriter {
    bits: Vec<bool>,
}
impl BitWriter {
    fn new() -> Self {
        Self { bits: Vec::new() }
    }
    fn put(&mut self, v: u32, n: u8) {
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

/// EC3SpecificBox (`dec3`) — ETSI TS 102 366 Annex F.6. Emits a single
/// independent substream descriptor (the common case), with `bsmod`/`asvc`/
/// dependent-substream fields left at their defaults.
fn dec3_box(h: &Eac3Header) -> Vec<u8> {
    // Nominal bit rate in kbit/s (13-bit field), derived from the frame geometry.
    let data_rate = ((h.frame_size as u64 * 8 * u64::from(h.sample_rate))
        / (u64::from(h.samples_per_frame.max(1)) * 1000))
        .min(0x1fff) as u32;

    let mut bw = BitWriter::new();
    bw.put(data_rate, 13);
    bw.put(0, 3); // num_ind_sub = 0 (one independent substream)
    // -- independent substream 0 --
    bw.put(u32::from(h.fscod), 2);
    bw.put(u32::from(h.bsid), 5);
    bw.put(0, 1); // reserved
    bw.put(0, 1); // asvc
    bw.put(0, 3); // bsmod
    bw.put(u32::from(h.acmod), 3);
    bw.put(u32::from(h.lfeon), 1);
    bw.put(0, 3); // reserved
    bw.put(0, 4); // num_dep_sub = 0
    bw.put(0, 1); // reserved (num_dep_sub == 0)
    let payload = bw.into_bytes();

    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    out.extend_from_slice(b"dec3");
    out.extend_from_slice(&payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eac3_frame() -> Vec<u8> {
        // Mirror of eac3::tests::frame(): 48k, 6 blocks, acmod 7, +LFE, 96 bytes.
        let bits: &[(u32, u8)] =
            &[(0, 2), (0, 3), (47, 11), (0, 2), (3, 2), (7, 3), (1, 1), (16, 5)];
        let mut bw = BitWriter::new();
        for &(v, n) in bits {
            bw.put(v, n);
        }
        let mut f = vec![0x0b, 0x77];
        f.extend_from_slice(&bw.into_bytes());
        f.resize(96, 0);
        f
    }

    #[test]
    fn builds_ec3_entry_with_dec3() {
        let entry = eac3_sample_entry(&eac3_frame()).expect("entry");
        assert_eq!(&entry[4..8], b"ec-3");
        // channelcount at body offset 16 → 6 (5.1).
        assert_eq!(u16::from_be_bytes([entry[8 + 16], entry[8 + 17]]), 6);
        // dec3 child box present with a 5-byte payload (single substream).
        let dec3_start = entry.len() - 13;
        assert_eq!(&entry[dec3_start + 4..dec3_start + 8], b"dec3");
        assert_eq!(entry.len() - dec3_start, 13); // 8 header + 5 payload
    }
}

//! Build an `ac-3` AudioSampleEntry (with its `dac3` box) from a syncframe header.

use crate::ac3::Ac3Header;

/// Build a complete `ac-3` AudioSampleEntry box (header included) from the first
/// AC-3 syncframe's BSI. Returns `None` when the syncframe cannot be parsed.
pub(crate) fn ac3_sample_entry(frame: &[u8]) -> Option<Vec<u8>> {
    let h = crate::ac3::parse_header(frame)?;
    let dac3 = dac3_box(&h);

    let mut out = Vec::new();
    // AudioSampleEntry: 28 fixed body bytes + child `dac3` box.
    let body_len = 28 + dac3.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(b"ac-3");
    out.extend_from_slice(&[0; 6]); // reserved (SampleEntry)
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 8]); // reserved (AudioSampleEntry)
    out.extend_from_slice(&u16::from(h.channels).to_be_bytes()); // channelcount
    out.extend_from_slice(&16u16.to_be_bytes()); // samplesize
    out.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&(h.sample_rate << 16).to_be_bytes()); // samplerate (16.16)
    out.extend_from_slice(&dac3);
    Some(out)
}

/// RFC 6381 codec string for AC-3 (no configuration suffix).
pub(crate) fn ac3_codec_string() -> String {
    "ac-3".to_string()
}

/// AC3SpecificBox (`dac3`) — ETSI TS 102 366 Annex F.4. The payload packs
/// `fscod(2) bsid(5) bsmod(3) acmod(3) lfeon(1) bit_rate_code(5) reserved(5)`
/// into 3 bytes, MSB-first.
fn dac3_box(h: &Ac3Header) -> Vec<u8> {
    let lfeon = u8::from(h.lfeon);
    let b0 = (h.fscod << 6) | (h.bsid << 1) | (h.bsmod >> 2);
    let b1 = ((h.bsmod & 0x03) << 6) | (h.acmod << 3) | (lfeon << 2) | (h.bit_rate_code >> 3);
    let b2 = (h.bit_rate_code & 0x07) << 5; // low 5 bits reserved (0)

    let mut out = Vec::with_capacity(11);
    out.extend_from_slice(&11u32.to_be_bytes()); // box size (8 header + 3 payload)
    out.extend_from_slice(b"dac3");
    out.extend_from_slice(&[b0, b1, b2]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Vec<u8> {
        let mut f = vec![0x0b, 0x77, 0x00, 0x00, 0x00, 0x40, 0x40];
        f.resize(128, 0);
        f
    }

    #[test]
    fn builds_ac3_entry_with_dac3() {
        let entry = ac3_sample_entry(&frame()).expect("entry");
        assert_eq!(&entry[4..8], b"ac-3");
        // channelcount (u16) at body offset 16 → 2 for acmod 2.
        assert_eq!(u16::from_be_bytes([entry[8 + 16], entry[8 + 17]]), 2);
        // 16.16 sample rate high word → 48000.
        let sr_off = 8 + 24;
        assert_eq!(u16::from_be_bytes([entry[sr_off], entry[sr_off + 1]]), 48_000);
        // Trailing child box is `dac3` with the packed BSI payload.
        let dac3 = &entry[entry.len() - 11..];
        assert_eq!(&dac3[4..8], b"dac3");
        assert_eq!(&dac3[8..11], &[0x10, 0x10, 0x00]);
    }
}

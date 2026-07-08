//! Build a `fLaC` AudioSampleEntry (with its `dfLa` box) from FLAC STREAMINFO.

/// Build a complete `fLaC` AudioSampleEntry box (header included) from a native
/// FLAC stream's STREAMINFO metadata block.
pub(crate) fn flac_sample_entry(data: &[u8]) -> Option<Vec<u8>> {
    let (si, streaminfo) = crate::flac::stream_info(data)?;
    let dfla = dfla_box(streaminfo);

    let mut out = Vec::new();
    let body_len = 28 + dfla.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(b"fLaC");
    out.extend_from_slice(&[0; 6]); // reserved (SampleEntry)
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 8]); // reserved (AudioSampleEntry)
    out.extend_from_slice(&u16::from(si.channels).to_be_bytes()); // channelcount
    out.extend_from_slice(&u16::from(si.bits_per_sample).to_be_bytes()); // samplesize
    out.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    // 16.16 samplerate — clamp to 16-bit integer part (dfLa carries the true rate).
    out.extend_from_slice(&(si.sample_rate.min(0xffff) << 16).to_be_bytes());
    out.extend_from_slice(&dfla);
    Some(out)
}

/// RFC 6381 codec string for FLAC.
pub(crate) fn flac_codec_string() -> String {
    "fLaC".to_string()
}

/// FLACSpecificBox (`dfLa`): a version/flags word followed by the STREAMINFO
/// metadata block (with a last-block header).
fn dfla_box(streaminfo: &[u8]) -> Vec<u8> {
    let mut block = Vec::with_capacity(4 + streaminfo.len());
    block.push(0x80); // last-metadata-block = 1, block type = 0 (STREAMINFO)
    let len = streaminfo.len() as u32;
    block.extend_from_slice(&len.to_be_bytes()[1..4]); // 24-bit length
    block.extend_from_slice(streaminfo);

    let mut out = Vec::with_capacity(12 + block.len());
    out.extend_from_slice(&((12 + block.len()) as u32).to_be_bytes());
    out.extend_from_slice(b"dfLa");
    out.extend_from_slice(&[0; 4]); // version + flags
    out.extend_from_slice(&block);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flac_stream() -> Vec<u8> {
        let mut si = [0u8; 34];
        si[0..2].copy_from_slice(&4096u16.to_be_bytes());
        si[2..4].copy_from_slice(&4096u16.to_be_bytes());
        let packed: u64 = (44_100u64 << 44) | (1u64 << 41) | (15u64 << 36);
        si[10..18].copy_from_slice(&packed.to_be_bytes());
        let mut out = Vec::new();
        out.extend_from_slice(crate::flac::MAGIC);
        out.push(0x80);
        out.extend_from_slice(&[0x00, 0x00, 0x22]);
        out.extend_from_slice(&si);
        out.extend_from_slice(&[0xff, 0xf8, 0, 0]);
        out
    }

    #[test]
    fn builds_flac_entry_with_dfla() {
        let entry = flac_sample_entry(&flac_stream()).expect("entry");
        assert_eq!(&entry[4..8], b"fLaC");
        assert_eq!(u16::from_be_bytes([entry[8 + 16], entry[8 + 17]]), 2); // channels
        assert_eq!(u16::from_be_bytes([entry[8 + 18], entry[8 + 19]]), 16); // bps
        // dfLa child box carrying a 34-byte STREAMINFO (4-byte block header).
        let dfla_start = entry.len() - (12 + 4 + 34);
        assert_eq!(&entry[dfla_start + 4..dfla_start + 8], b"dfLa");
        assert_eq!(flac_codec_string(), "fLaC");
    }
}

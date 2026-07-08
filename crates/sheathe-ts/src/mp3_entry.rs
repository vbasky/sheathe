//! Build an `mp4a` AudioSampleEntry for MP3 (ISO/IEC 14496-1 OTI `0x6B`/`0x69`).

/// Build a complete `mp4a` AudioSampleEntry (header included) wrapping an MP3
/// stream, with an `esds` carrying the MPEG-1/2 Audio object type indication.
pub(crate) fn mp3_sample_entry(frame: &[u8]) -> Option<Vec<u8>> {
    let h = crate::mp3::parse_header(frame)?;
    let oti = if h.is_mpeg2 { 0x69 } else { 0x6b };
    let esds = build_esds(oti);

    let mut out = Vec::new();
    let body_len = 28 + esds.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(b"mp4a");
    out.extend_from_slice(&[0; 6]); // reserved (SampleEntry)
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 8]); // reserved (AudioSampleEntry)
    out.extend_from_slice(&u16::from(h.channels).to_be_bytes()); // channelcount
    out.extend_from_slice(&16u16.to_be_bytes()); // samplesize
    out.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&(h.sample_rate << 16).to_be_bytes()); // samplerate (16.16)
    out.extend_from_slice(&esds);
    Some(out)
}

/// RFC 6381 codec string for MP3 (`mp4a.6B` / `mp4a.69`).
pub(crate) fn mp3_codec_string(frame: &[u8]) -> Option<String> {
    let h = crate::mp3::parse_header(frame)?;
    Some(if h.is_mpeg2 { "mp4a.69".to_string() } else { "mp4a.6b".to_string() })
}

/// An `esds` with a DecoderConfigDescriptor for MPEG audio and **no**
/// DecoderSpecificInfo (MP3 needs none), streamType = audio (0x05).
fn build_esds(oti: u8) -> Vec<u8> {
    let mut dcd = vec![
        0x04,               // DecoderConfigDescriptor
        0x0d,               // length
        oti,                // objectTypeIndication
        (0x05 << 2) | 0x01, // streamType=audio(5), upStream=0, reserved=1
    ];
    dcd.extend_from_slice(&[0x00, 0x00, 0x00]); // bufferSizeDB
    dcd.extend_from_slice(&0u32.to_be_bytes()); // maxBitrate
    dcd.extend_from_slice(&0u32.to_be_bytes()); // avgBitrate

    let mut es = Vec::new();
    es.push(0x03); // ES_Descriptor
    es.push((3 + dcd.len() + 3) as u8); // length
    es.extend_from_slice(&0u16.to_be_bytes()); // ES_ID
    es.push(0x00); // flags
    es.extend_from_slice(&dcd);
    es.extend_from_slice(&[0x06, 0x01, 0x02]); // SLConfigDescriptor

    let mut out = Vec::new();
    out.extend_from_slice(&(8u32 + 4 + es.len() as u32).to_be_bytes());
    out.extend_from_slice(b"esds");
    out.extend_from_slice(&[0; 4]); // version + flags
    out.extend_from_slice(&es);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Vec<u8> {
        let mut f = vec![0xff, 0xfb, 0x90, 0x00];
        f.resize(crate::mp3::parse_header(&f).unwrap().frame_size, 0);
        f
    }

    #[test]
    fn builds_mp4a_entry_for_mp3() {
        let entry = mp3_sample_entry(&frame()).expect("entry");
        assert_eq!(&entry[4..8], b"mp4a");
        assert_eq!(u16::from_be_bytes([entry[8 + 16], entry[8 + 17]]), 2); // stereo
        // esds present with OTI 0x6b somewhere in the DCD.
        assert!(entry.windows(1).any(|w| w == [0x6b]));
        assert_eq!(mp3_codec_string(&frame()).as_deref(), Some("mp4a.6b"));
    }
}

//! Build an `mp4a` sample entry from ADTS AAC frames.

/// Build a complete `mp4a` AudioSampleEntry box (header included) from the first ADTS frame.
pub(crate) fn mp4a_sample_entry(adts_frame: &[u8]) -> Option<Vec<u8>> {
    if adts_frame.len() < 7 || adts_frame[0] != 0xff || (adts_frame[1] & 0xf0) != 0xf0 {
        return None;
    }
    let profile = (adts_frame[2] >> 6) + 1;
    let sr_idx = (adts_frame[2] & 0x3c) >> 2;
    let channels = ((adts_frame[2] & 0x01) << 2) | (adts_frame[3] >> 6);
    let sample_rate = super::adts::ADTS_SAMPLE_RATES.get(usize::from(sr_idx)).copied()?;
    if sample_rate == 0 {
        return None;
    }

    // AudioSpecificConfig (2 bytes for AAC-LC).
    let asc = [(profile << 3) | (sr_idx >> 1), ((sr_idx & 1) << 7) | (channels << 3)];
    let esds = build_esds(&asc);

    let mut out = Vec::new();
    let body_len = 28 + esds.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(b"mp4a");
    out.extend_from_slice(&[0; 6]);
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 8]); // reserved
    out.extend_from_slice(&channels.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&(sample_rate << 16).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // reserved
    out.extend_from_slice(&esds);
    Some(out)
}

/// RFC 6381 mp4a.40.N from ADTS profile.
pub(crate) fn aac_codec_string(adts_frame: &[u8]) -> Option<String> {
    if adts_frame.len() < 3 || adts_frame[0] != 0xff {
        return None;
    }
    let profile = (adts_frame[2] >> 6) + 1;
    Some(format!("mp4a.40.{profile}"))
}

fn build_esds(asc: &[u8]) -> Vec<u8> {
    let mut esds_body = Vec::new();
    esds_body.extend_from_slice(&[0x03, 0x17, 0x00, 0x01]); // ES_Descriptor
    esds_body.extend_from_slice(&[0x04, 0x0f, 0x40, 0x15]); // placeholder DCD header
    esds_body.extend_from_slice(&0u32.to_be_bytes());
    esds_body.extend_from_slice(&0u32.to_be_bytes());
    esds_body.push(0x05);
    esds_body.push(asc.len() as u8);
    esds_body.extend_from_slice(asc);
    esds_body.push(0x06);
    esds_body.push(0x01);
    esds_body.push(0x02);

    let mut out = Vec::new();
    out.extend_from_slice(&(8u32 + esds_body.len() as u32).to_be_bytes());
    out.extend_from_slice(b"esds");
    out.extend_from_slice(&[0; 4]); // version + flags
    out.extend_from_slice(&esds_body);
    out
}

//! Build an `avc1` sample entry + `avcC` from H.264 SPS/PPS NAL units.

/// Build a complete `avc1` VisualSampleEntry box (header included) from SPS/PPS.
pub(crate) fn avc1_sample_entry(sps: &[u8], pps: &[u8], width: u16, height: u16) -> Vec<u8> {
    let avcc = build_avcc(sps, pps);
    let mut out = Vec::new();
    let body_len = 78 + 8 + avcc.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(b"avc1");
    out.extend_from_slice(&[0; 6]); // reserved
    out.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    out.extend_from_slice(&[0; 8]); // pre_defined + reserved
    out.extend_from_slice(&[0; 12]); // pre_defined[3]
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // horizresolution
    out.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // vertresolution
    out.extend_from_slice(&0u32.to_be_bytes()); // reserved
    out.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    out.extend_from_slice(&[0; 32]); // compressorname
    out.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
    out.extend_from_slice(&0xffffu16.to_be_bytes()); // pre_defined = -1
    out.extend_from_slice(&(8u32 + avcc.len() as u32).to_be_bytes());
    out.extend_from_slice(b"avcC");
    out.extend_from_slice(&avcc);
    out
}

fn build_avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let profile = sps.get(1).copied().unwrap_or(0x42);
    let compat = sps.get(2).copied().unwrap_or(0);
    let level = sps.get(3).copied().unwrap_or(0x1f);
    let mut out = vec![
        1, // configurationVersion
        profile,
        compat,
        level,
        0xfc | 3, // lengthSizeMinusOne = 3 (4-byte NAL lengths)
        0xe1,     // numOfSequenceParameterSets = 1
    ];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(1); // numOfPictureParameterSets
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    out
}

/// Derive RFC 6381 `avc1.PPCCLL` from SPS bytes.
pub(crate) fn avc_codec_string(sps: &[u8]) -> Option<String> {
    if sps.len() < 4 {
        return None;
    }
    Some(format!("avc1.{:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]))
}
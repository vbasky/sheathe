//! Build an `hvc1` sample entry + `hvcC` from HEVC VPS/SPS/PPS NAL units.

use crate::bitstream::{BitReader, rbsp_from_nal};

/// Profile/tier/level fields extracted from an HEVC SPS.
#[derive(Debug, Clone, Copy)]
struct ProfileTierLevel {
    profile_space: u8,
    tier_flag: u8,
    profile_idc: u8,
    compatibility: u32,
    constraint: [u8; 6],
    level_idc: u8,
}

/// Build an `hvcC` configuration record from VPS/SPS/PPS NAL units.
pub(crate) fn hvcc_bytes(vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
    build_hvcc(vps, sps, pps)
}

/// Build a complete `hvc1` VisualSampleEntry box (header included) from VPS/SPS/PPS.
pub(crate) fn hvc1_sample_entry(
    vps: &[u8],
    sps: &[u8],
    pps: &[u8],
    width: u16,
    height: u16,
) -> Vec<u8> {
    let hvcc = build_hvcc(vps, sps, pps);
    let mut out = Vec::new();
    let body_len = 78 + 8 + hvcc.len();
    out.extend_from_slice(&(8u32 + body_len as u32).to_be_bytes());
    out.extend_from_slice(b"hvc1");
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
    out.extend_from_slice(&(8u32 + hvcc.len() as u32).to_be_bytes());
    out.extend_from_slice(b"hvcC");
    out.extend_from_slice(&hvcc);
    out
}

fn build_hvcc(vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let ptl = parse_sps_profile_tier_level(sps).unwrap_or(ProfileTierLevel {
        profile_space: 0,
        tier_flag: 0,
        profile_idc: 1,
        compatibility: 0x6000_0000,
        constraint: [0x90, 0, 0, 0, 0, 0],
        level_idc: 0x5d,
    });

    let mut out = vec![
        1, // configurationVersion
        (ptl.profile_space << 6) | (ptl.tier_flag << 5) | (ptl.profile_idc & 0x1f),
    ];
    out.extend_from_slice(&ptl.compatibility.to_be_bytes());
    out.extend_from_slice(&ptl.constraint);
    out.push(ptl.level_idc);
    // min_spatial_segmentation_idc (12 bits) + reserved (4 bits)
    out.extend_from_slice(&0xf000u16.to_be_bytes());
    out.push(0xfc); // parallelismType = 0 + reserved
    out.push(0xfc | 1); // chromaFormat = 1 + reserved
    out.push(0xf8); // bitDepthLumaMinus8 = 0 + reserved
    out.push(0xf8); // bitDepthChromaMinus8 = 0 + reserved
    out.extend_from_slice(&0u16.to_be_bytes()); // avgFrameRate
    // constantFrameRate(2) + numTemporalLayers(3) + temporalIdNested(1) + lengthSizeMinusOne(2)
    out.push(0x0f); // lengthSizeMinusOne = 3
    out.push(3); // numOfArrays

    for (complete, nal_type, nal) in [(1u8, 32u8, vps), (1, 33, sps), (1, 34, pps)] {
        out.push((complete << 7) | (nal_type & 0x3f));
        out.extend_from_slice(&1u16.to_be_bytes()); // numNalus
        out.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

fn parse_sps_profile_tier_level(sps: &[u8]) -> Option<ProfileTierLevel> {
    // HEVC NAL header is 2 bytes.
    let rbsp = rbsp_from_nal(sps.get(2..)?);
    let mut br = BitReader::new(&rbsp);
    let _sps_video_parameter_set_id = br.read_ue()?;
    let sps_max_sub_layers_minus1 = br.read_u(3)? as u8;
    let _sps_temporal_id_nesting_flag = br.read_u1()?;
    parse_profile_tier_level(&mut br, sps_max_sub_layers_minus1)
}

fn parse_profile_tier_level(
    br: &mut BitReader<'_>,
    max_sub_layers_minus1: u8,
) -> Option<ProfileTierLevel> {
    let profile_space = br.read_u(2)? as u8;
    let tier_flag = br.read_u1()? as u8;
    let profile_idc = br.read_u(5)? as u8;
    let mut compat = 0u32;
    for i in 0..32 {
        if br.read_u1()? != 0 {
            compat |= 1 << (31 - i);
        }
    }
    let progressive = br.read_u1()?;
    let interlaced = br.read_u1()?;
    let non_packed = br.read_u1()?;
    let frame_only = br.read_u1()?;
    let mut constraint = [0u8; 6];
    constraint[0] =
        ((progressive << 7) | (interlaced << 6) | (non_packed << 5) | (frame_only << 4)) as u8;
    // Remaining constraint bits depend on profile; for Main (1) read 44 reserved zero bits.
    if (1..=3u8).contains(&profile_idc) || profile_idc == 11 {
        for _ in 0..44 {
            let _ = br.read_u1()?;
        }
    }
    let level_idc = br.read_u(8)? as u8;
    let sub_layers = usize::from(max_sub_layers_minus1);
    let mut profile_present = vec![false; sub_layers];
    let mut level_present = vec![false; sub_layers];
    for flag in &mut profile_present {
        *flag = br.read_u1()? != 0;
    }
    for flag in &mut level_present {
        *flag = br.read_u1()? != 0;
    }
    for i in 0..sub_layers {
        if profile_present[i] {
            parse_sub_layer_profile_tier_level(br)?;
        }
        if level_present[i] {
            let _ = br.read_u(8)?;
        }
    }
    Some(ProfileTierLevel {
        profile_space,
        tier_flag,
        profile_idc,
        compatibility: compat,
        constraint,
        level_idc,
    })
}

fn parse_sub_layer_profile_tier_level(br: &mut BitReader<'_>) -> Option<()> {
    let _profile_space = br.read_u(2)?;
    let _tier_flag = br.read_u1()?;
    let profile_idc = br.read_u(5)? as u8;
    for _ in 0..32 {
        let _ = br.read_u1()?;
    }
    let _ = br.read_u1()?;
    let _ = br.read_u1()?;
    let _ = br.read_u1()?;
    let _ = br.read_u1()?;
    if (1..=3u8).contains(&profile_idc) || profile_idc == 11 {
        for _ in 0..44 {
            let _ = br.read_u1()?;
        }
    }
    Some(())
}

/// Derive RFC 6381 `hvc1.A.B.C.D` from an `hvcC`-like byte sequence.
pub(crate) fn hevc_codec_string(hvcc: &[u8]) -> Option<String> {
    if hvcc.len() < 13 {
        return None;
    }
    let b1 = hvcc[1];
    let profile_space = (b1 >> 6) & 0x3;
    let tier = (b1 >> 5) & 0x1;
    let profile_idc = b1 & 0x1f;
    let compat = u32::from_be_bytes([hvcc[2], hvcc[3], hvcc[4], hvcc[5]]);
    let constraint = &hvcc[6..12];
    let level = hvcc[12];

    let space = match profile_space {
        1 => "A",
        2 => "B",
        3 => "C",
        _ => "",
    };
    let tier_char = if tier == 0 { 'L' } else { 'H' };
    let mut s = format!("hvc1.{space}{profile_idc}.{:X}.{tier_char}{level}", compat.reverse_bits());

    let last = constraint.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    for byte in &constraint[..last] {
        s.push_str(&format!(".{byte:02X}"));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_string_matches_mp4_convention() {
        let hvcc = [1u8, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0, 0, 0, 0, 0, 0x5d];
        assert_eq!(hevc_codec_string(&hvcc).as_deref(), Some("hvc1.1.6.L93.90"));
    }
}

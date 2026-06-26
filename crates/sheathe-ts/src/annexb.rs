//! H.264/H.265 Annex B NAL parsing and access-unit helpers.

use sheathe_core::{Sample, SampleFlags};

/// NAL unit types in H.264.
const AVC_NAL_IDR: u8 = 5;
const AVC_NAL_SPS: u8 = 7;
const AVC_NAL_PPS: u8 = 8;

/// NAL unit types in HEVC (ITU-T H.265 Table 7-1).
const HEVC_NAL_VPS: u8 = 32;
const HEVC_NAL_SPS: u8 = 33;
const HEVC_NAL_PPS: u8 = 34;
const HEVC_NAL_IDR_W_RADL: u8 = 19;
const HEVC_NAL_IDR_N_LP: u8 = 20;

/// Wrap one H.264 PES payload (Annex B) as a single sample.
pub(crate) fn pes_sample(data: &[u8], pts: u64, dts: u64, duration: u32) -> Sample {
    let keyframe =
        split_nals(data).iter().any(|n| n.first().map(|b| b & 0x1f) == Some(AVC_NAL_IDR));
    let mut flags = SampleFlags::empty();
    if keyframe {
        flags.insert(SampleFlags::KEYFRAME);
    }
    Sample { dts, pts, duration, flags, data: data.to_vec() }
}

/// Extract the first SPS and PPS NAL units (without start codes) from H.264 Annex B data.
pub(crate) fn parameter_sets(data: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let mut sps = None;
    let mut pps = None;
    for nal in split_nals(data) {
        let nal_type = nal.first().map(|b| b & 0x1f).unwrap_or(0);
        match nal_type {
            AVC_NAL_SPS if sps.is_none() => sps = Some(nal.to_vec()),
            AVC_NAL_PPS if pps.is_none() => pps = Some(nal.to_vec()),
            _ => {}
        }
        if sps.is_some() && pps.is_some() {
            break;
        }
    }
    (sps, pps)
}

/// HEVC parameter sets `(VPS, SPS, PPS)`, each without its Annex B start code.
pub(crate) type HevcParameterSets = (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);

/// Extract the first VPS/SPS/PPS NAL units (without start codes) from HEVC Annex B data.
pub(crate) fn hevc_parameter_sets(data: &[u8]) -> HevcParameterSets {
    let mut vps = None;
    let mut sps = None;
    let mut pps = None;
    for nal in split_nals(data) {
        let nal_type = hevc_nal_type(nal);
        match nal_type {
            HEVC_NAL_VPS if vps.is_none() => vps = Some(nal.to_vec()),
            HEVC_NAL_SPS if sps.is_none() => sps = Some(nal.to_vec()),
            HEVC_NAL_PPS if pps.is_none() => pps = Some(nal.to_vec()),
            _ => {}
        }
        if vps.is_some() && sps.is_some() && pps.is_some() {
            break;
        }
    }
    (vps, sps, pps)
}

/// Wrap one HEVC PES payload (Annex B) as a single sample.
pub(crate) fn hevc_pes_sample(data: &[u8], pts: u64, dts: u64, duration: u32) -> Sample {
    let keyframe = split_nals(data)
        .iter()
        .any(|n| matches!(hevc_nal_type(n), HEVC_NAL_IDR_W_RADL | HEVC_NAL_IDR_N_LP));
    let mut flags = SampleFlags::empty();
    if keyframe {
        flags.insert(SampleFlags::KEYFRAME);
    }
    Sample { dts, pts, duration, flags, data: data.to_vec() }
}

fn hevc_nal_type(nal: &[u8]) -> u8 {
    nal.first().map(|b| (b >> 1) & 0x3f).unwrap_or(0)
}

/// Split a raw Annex B H.264 stream into access units (one per slice/IDR NAL).
pub(crate) fn avc_access_units(data: &[u8]) -> Vec<Vec<u8>> {
    access_units(data, |nal| {
        let ty = nal.first().map(|b| b & 0x1f).unwrap_or(0);
        matches!(ty, 1 | 5)
    })
}

/// Split a raw Annex B HEVC stream into access units (one per VCL NAL).
pub(crate) fn hevc_access_units(data: &[u8]) -> Vec<Vec<u8>> {
    access_units(data, |nal| hevc_nal_type(nal) <= 31)
}

fn access_units(data: &[u8], is_vcl: impl Fn(&[u8]) -> bool) -> Vec<Vec<u8>> {
    let nals: Vec<Vec<u8>> = split_nals(data).into_iter().map(|n| n.to_vec()).collect();
    let mut aus = Vec::new();
    let mut prefix = Vec::new();
    let mut current = Vec::new();

    for nal in nals {
        if is_vcl(&nal) {
            if !current.is_empty() {
                aus.push(std::mem::take(&mut current));
            }
            if aus.is_empty() && !prefix.is_empty() {
                current.append(&mut prefix);
            }
            append_nal(&mut current, &nal);
        } else if aus.is_empty() && current.is_empty() {
            append_nal(&mut prefix, &nal);
        } else {
            append_nal(&mut current, &nal);
        }
    }
    if !current.is_empty() {
        aus.push(current);
    } else if !prefix.is_empty() {
        aus.push(prefix);
    }
    aus
}

fn append_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

/// Split Annex B data into individual NAL units (without start codes).
pub(crate) fn split_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let start = if data[i..].starts_with(&[0, 0, 0, 1]) {
            i + 4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            i + 3
        } else {
            i += 1;
            continue;
        };
        let mut end = data.len();
        for j in start..data.len() {
            if data[j..].starts_with(&[0, 0, 1]) || data[j..].starts_with(&[0, 0, 0, 1]) {
                end = j;
                break;
            }
        }
        if start < end {
            out.push(&data[start..end]);
        }
        i = end;
    }
    out
}

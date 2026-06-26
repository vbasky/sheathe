//! Elementary stream track building (shared by MPEG-TS and raw ES demuxers).

use crate::aac_entry::{aac_codec_string, mp4a_sample_entry};
use crate::adts::{frames as adts_frames, sample_rate_hz};
use crate::annexb::{
    avc_access_units, hevc_access_units, hevc_parameter_sets, hevc_pes_sample, parameter_sets,
    pes_sample,
};
use crate::avcc::{avc1_sample_entry, avc_codec_string};
use crate::hvcc::{hevc_codec_string, hvc1_sample_entry, hvcc_bytes};
use sheathe_core::{Codec, Error, MediaKind, Result, Sample, StreamInfo, Timescale};

/// Default video frame duration in 90 kHz ticks (25 fps).
pub const DEFAULT_VIDEO_TICKS: u32 = 3_600;

/// A demuxed elementary stream track.
#[derive(Debug, Clone)]
pub struct ElementaryTrack {
    /// Format-agnostic stream description.
    pub info: StreamInfo,
    /// Demuxed samples in decode order.
    pub samples: Vec<Sample>,
    /// `avc1`/`hvc1`/`mp4a` sample-entry box bytes for CMAF init segments.
    pub sample_entry: Vec<u8>,
}

/// Build an H.264 track from MPEG-TS PES payloads (Annex B per PES).
pub fn h264_from_pes(pes_payloads: &[(&[u8], Option<u64>, Option<u64>)]) -> Result<ElementaryTrack> {
    let all_data: Vec<u8> = pes_payloads.iter().flat_map(|(d, _, _)| d.iter().copied()).collect();
    h264_from_annex_b(&all_data, pes_payloads)
}

/// Build an H.264 track from a raw Annex B byte stream (access units inferred).
pub fn h264_from_annex_b(
    all_data: &[u8],
    pes_payloads: &[(&[u8], Option<u64>, Option<u64>)],
) -> Result<ElementaryTrack> {
    let (sps, pps) = parameter_sets(all_data);
    let (Some(sps), Some(pps)) = (sps, pps) else {
        return Err(Error::malformed("H.264: missing SPS/PPS in elementary stream"));
    };
    let sample_entry = avc1_sample_entry(&sps, &pps, 640, 360);
    let info = StreamInfo {
        kind: MediaKind::Video,
        codec: Codec::H264,
        timescale: Timescale::MPEG_TS,
        resolution: Some((640, 360)),
        sample_rate: None,
        bitrate: None,
        codec_string: avc_codec_string(&sps),
    };

    let samples = if pes_payloads.is_empty() {
        samples_from_access_units(&avc_access_units(all_data), DEFAULT_VIDEO_TICKS, pes_sample)
    } else {
        samples_from_pes(pes_payloads, pes_sample)
    };
    if samples.is_empty() {
        return Err(Error::malformed("H.264: no samples in elementary stream"));
    }
    Ok(ElementaryTrack { info, samples, sample_entry })
}

/// Build an HEVC track from MPEG-TS PES payloads.
pub fn hevc_from_pes(pes_payloads: &[(&[u8], Option<u64>, Option<u64>)]) -> Result<ElementaryTrack> {
    let all_data: Vec<u8> = pes_payloads.iter().flat_map(|(d, _, _)| d.iter().copied()).collect();
    hevc_from_annex_b(&all_data, pes_payloads)
}

/// Build an HEVC track from a raw Annex B byte stream.
pub fn hevc_from_annex_b(
    all_data: &[u8],
    pes_payloads: &[(&[u8], Option<u64>, Option<u64>)],
) -> Result<ElementaryTrack> {
    let (vps, sps, pps) = hevc_parameter_sets(all_data);
    let (Some(vps), Some(sps), Some(pps)) = (vps, sps, pps) else {
        return Err(Error::malformed("HEVC: missing VPS/SPS/PPS in elementary stream"));
    };
    let sample_entry = hvc1_sample_entry(&vps, &sps, &pps, 640, 360);
    let info = StreamInfo {
        kind: MediaKind::Video,
        codec: Codec::H265,
        timescale: Timescale::MPEG_TS,
        resolution: Some((640, 360)),
        sample_rate: None,
        bitrate: None,
        codec_string: hevc_codec_string(&hvcc_bytes(&vps, &sps, &pps)),
    };

    let samples = if pes_payloads.is_empty() {
        samples_from_access_units(&hevc_access_units(all_data), DEFAULT_VIDEO_TICKS, hevc_pes_sample)
    } else {
        samples_from_pes(pes_payloads, hevc_pes_sample)
    };
    if samples.is_empty() {
        return Err(Error::malformed("HEVC: no samples in elementary stream"));
    }
    Ok(ElementaryTrack { info, samples, sample_entry })
}

/// Build an ADTS-AAC track from a byte stream.
pub fn aac_adts(data: &[u8]) -> Result<ElementaryTrack> {
    let sample_rate = sample_rate_hz(data)
        .ok_or_else(|| Error::malformed("AAC: could not read ADTS sample rate"))?;
    let samples = adts_frames(data, 0, 0, sample_rate);
    let Some(first) = samples.first() else {
        return Err(Error::malformed("AAC: no ADTS frames found"));
    };
    let sample_entry = mp4a_sample_entry(&first.data)
        .ok_or_else(|| Error::malformed("AAC: could not build mp4a sample entry"))?;
    let info = StreamInfo {
        kind: MediaKind::Audio,
        codec: Codec::Aac,
        timescale: Timescale::MPEG_TS,
        resolution: None,
        sample_rate: Some(sample_rate),
        bitrate: None,
        codec_string: aac_codec_string(&first.data),
    };
    Ok(ElementaryTrack { info, samples, sample_entry })
}

fn samples_from_pes(
    pes_payloads: &[(&[u8], Option<u64>, Option<u64>)],
    make_sample: fn(&[u8], u64, u64, u32) -> Sample,
) -> Vec<Sample> {
    let mut samples = Vec::new();
    for (i, (data, pts, dts)) in pes_payloads.iter().enumerate() {
        if data.is_empty() {
            continue;
        }
        let pts = pts.unwrap_or(i as u64 * u64::from(DEFAULT_VIDEO_TICKS));
        let dts = dts.unwrap_or(pts);
        samples.push(make_sample(data, pts, dts, DEFAULT_VIDEO_TICKS));
    }
    samples
}

fn samples_from_access_units(
    access_units: &[Vec<u8>],
    duration: u32,
    make_sample: fn(&[u8], u64, u64, u32) -> Sample,
) -> Vec<Sample> {
    access_units
        .iter()
        .enumerate()
        .map(|(i, au)| {
            let ticks = i as u64 * u64::from(duration);
            make_sample(au, ticks, ticks, duration)
        })
        .collect()
}
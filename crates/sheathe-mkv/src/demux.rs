//! WebM/Matroska demuxer: EBML → tracks + samples.

use crate::{ebml, entries};
use sheathe_core::{Codec, Error, MediaKind, Result, Sample, SampleFlags, StreamInfo, Timescale};

// EBML element IDs (with length-marker bits retained).
const ID_SEGMENT: u64 = 0x1853_8067;
const ID_INFO: u64 = 0x1549_A966;
const ID_TIMESTAMP_SCALE: u64 = 0x002A_D7B1;
const ID_TRACKS: u64 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u64 = 0xAE;
const ID_TRACK_NUMBER: u64 = 0xD7;
const ID_TRACK_TYPE: u64 = 0x83;
const ID_CODEC_ID: u64 = 0x86;
const ID_CODEC_PRIVATE: u64 = 0x63A2;
const ID_VIDEO: u64 = 0xE0;
const ID_PIXEL_WIDTH: u64 = 0xB0;
const ID_PIXEL_HEIGHT: u64 = 0xBA;
const ID_AUDIO: u64 = 0xE1;
const ID_SAMPLING_FREQ: u64 = 0xB5;
const ID_CHANNELS: u64 = 0x9F;
const ID_DEFAULT_DURATION: u64 = 0x23E383;
const ID_CLUSTER: u64 = 0x1F43_B675;
const ID_CLUSTER_TIMESTAMP: u64 = 0xE7;
const ID_SIMPLE_BLOCK: u64 = 0xA3;
const ID_BLOCK_GROUP: u64 = 0xA0;
const ID_BLOCK: u64 = 0xA1;
const ID_REFERENCE_BLOCK: u64 = 0xFB;

const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;

/// A demuxed WebM/Matroska track.
pub struct MkvTrack {
    /// Format-agnostic stream description.
    pub info: StreamInfo,
    /// Demuxed samples in decode order.
    pub samples: Vec<Sample>,
    /// Sample-entry box bytes for the CMAF init segment.
    pub sample_entry: Vec<u8>,
}

/// A demuxed WebM/Matroska file.
pub struct MkvDemuxer {
    tracks: Vec<MkvTrack>,
}

/// EBML magic — the first bytes of any Matroska / WebM file.
pub fn is_webm(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..4] == [0x1a, 0x45, 0xdf, 0xa3]
}

struct TrackDef {
    number: u64,
    kind: MediaKind,
    codec_id: String,
    codec_private: Vec<u8>,
    width: u16,
    height: u16,
    channels: u16,
    sample_rate: u32,
    default_duration_ns: u64,
}

impl MkvDemuxer {
    /// Parse a WebM/Matroska byte stream.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if !is_webm(data) {
            return Err(Error::malformed("not an EBML/WebM stream"));
        }
        // Find the Segment element at the top level.
        let segment = ebml::children(data)
            .into_iter()
            .find(|(id, _)| *id == ID_SEGMENT)
            .map(|(_, body)| body)
            .ok_or_else(|| Error::malformed("WebM: no Segment element"))?;

        let mut timestamp_scale = 1_000_000u64; // ns per tick (Matroska default)
        let mut defs: Vec<TrackDef> = Vec::new();
        let mut clusters: Vec<&[u8]> = Vec::new();

        for (id, body) in ebml::children(segment) {
            match id {
                ID_INFO => {
                    for (cid, cbody) in ebml::children(body) {
                        if cid == ID_TIMESTAMP_SCALE {
                            timestamp_scale = ebml::as_uint(cbody).max(1);
                        }
                    }
                }
                ID_TRACKS => {
                    for (tid, tbody) in ebml::children(body) {
                        if tid == ID_TRACK_ENTRY {
                            if let Some(def) = parse_track_entry(tbody) {
                                defs.push(def);
                            }
                        }
                    }
                }
                ID_CLUSTER => clusters.push(body),
                _ => {}
            }
        }

        // Collect raw (track_number, pts_ticks, keyframe, data) across clusters.
        let mut per_track: std::collections::BTreeMap<u64, Vec<Sample>> =
            std::collections::BTreeMap::new();
        for cluster in clusters {
            parse_cluster(cluster, timestamp_scale, &mut per_track);
        }

        let mut tracks = Vec::new();
        for def in &defs {
            let Some(mut samples) = per_track.remove(&def.number) else { continue };
            if samples.is_empty() {
                continue;
            }
            fill_durations(&mut samples, def.default_duration_ns, timestamp_scale);
            let Some((sample_entry, info)) = build_stream(def, &samples) else { continue };
            tracks.push(MkvTrack { info, samples, sample_entry });
        }

        if tracks.is_empty() {
            return Err(Error::malformed("WebM: no supported tracks found"));
        }
        Ok(Self { tracks })
    }

    /// The demuxed tracks.
    pub fn tracks(&self) -> &[MkvTrack] {
        &self.tracks
    }
}

fn parse_track_entry(body: &[u8]) -> Option<TrackDef> {
    let mut def = TrackDef {
        number: 0,
        kind: MediaKind::Video,
        codec_id: String::new(),
        codec_private: Vec::new(),
        width: 0,
        height: 0,
        channels: 2,
        sample_rate: 48_000,
        default_duration_ns: 0,
    };
    let mut track_type = 0u64;
    for (id, b) in ebml::children(body) {
        match id {
            ID_TRACK_NUMBER => def.number = ebml::as_uint(b),
            ID_TRACK_TYPE => track_type = ebml::as_uint(b),
            ID_CODEC_ID => def.codec_id = String::from_utf8_lossy(b).into_owned(),
            ID_CODEC_PRIVATE => def.codec_private = b.to_vec(),
            ID_DEFAULT_DURATION => def.default_duration_ns = ebml::as_uint(b),
            ID_VIDEO => {
                for (vid, vb) in ebml::children(b) {
                    match vid {
                        ID_PIXEL_WIDTH => def.width = ebml::as_uint(vb) as u16,
                        ID_PIXEL_HEIGHT => def.height = ebml::as_uint(vb) as u16,
                        _ => {}
                    }
                }
            }
            ID_AUDIO => {
                for (aid, ab) in ebml::children(b) {
                    match aid {
                        ID_SAMPLING_FREQ => def.sample_rate = ebml::as_float(ab) as u32,
                        ID_CHANNELS => def.channels = ebml::as_uint(ab) as u16,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    def.kind = match track_type {
        TRACK_TYPE_VIDEO => MediaKind::Video,
        TRACK_TYPE_AUDIO => MediaKind::Audio,
        _ => return None, // subtitle / other track types unsupported here
    };
    if def.number == 0 {
        return None;
    }
    Some(def)
}

/// Read a bare EBML var-int (size form, marker stripped) at `data[off..]`.
fn read_vint(data: &[u8], off: usize) -> Option<(u64, usize)> {
    let first = *data.get(off)?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if off + len > data.len() {
        return None;
    }
    let mut val = u64::from(first & (0xff >> len));
    for i in 1..len {
        val = (val << 8) | u64::from(data[off + i]);
    }
    Some((val, len))
}

fn parse_cluster(
    body: &[u8],
    timestamp_scale: u64,
    per_track: &mut std::collections::BTreeMap<u64, Vec<Sample>>,
) {
    let mut cluster_ts = 0u64;
    for (id, b) in ebml::children(body) {
        match id {
            ID_CLUSTER_TIMESTAMP => cluster_ts = ebml::as_uint(b),
            ID_SIMPLE_BLOCK => emit_block(b, cluster_ts, timestamp_scale, None, per_track),
            ID_BLOCK_GROUP => {
                let mut keyframe = true; // no ReferenceBlock ⇒ keyframe
                let mut block: Option<&[u8]> = None;
                for (gid, gb) in ebml::children(b) {
                    match gid {
                        ID_BLOCK => block = Some(gb),
                        ID_REFERENCE_BLOCK => keyframe = false,
                        _ => {}
                    }
                }
                if let Some(blk) = block {
                    emit_block(blk, cluster_ts, timestamp_scale, Some(keyframe), per_track);
                }
            }
            _ => {}
        }
    }
}

/// Decode a (Simple)Block and push its frame(s) to the matching track.
fn emit_block(
    block: &[u8],
    cluster_ts: u64,
    timestamp_scale: u64,
    force_keyframe: Option<bool>,
    per_track: &mut std::collections::BTreeMap<u64, Vec<Sample>>,
) {
    let Some((track, tlen)) = read_vint(block, 0) else { return };
    let rest = &block[tlen..];
    if rest.len() < 3 {
        return;
    }
    let rel_ts = i16::from_be_bytes([rest[0], rest[1]]);
    let flags = rest[2];
    let payload = &rest[3..];
    let keyframe = force_keyframe.unwrap_or(flags & 0x80 != 0);

    let abs_ts = (cluster_ts as i64 + i64::from(rel_ts)).max(0) as u64;
    let pts = scale_to_90k(abs_ts, timestamp_scale);

    let flag = if keyframe { SampleFlags::KEYFRAME } else { SampleFlags::empty() };
    let entry = per_track.entry(track).or_default();
    for frame in split_lacing(payload, (flags >> 1) & 0x03) {
        entry.push(Sample { dts: pts, pts, duration: 0, flags: flag, data: frame.to_vec() });
    }
}

/// Split a block payload into frames per the lacing mode (0 none, 2 fixed).
/// Xiph (1) and EBML (3) lacing fall back to a single frame.
fn split_lacing(payload: &[u8], lacing: u8) -> Vec<&[u8]> {
    if lacing == 2 && !payload.is_empty() {
        // Fixed-size lacing: [frame_count-1][equal-size frames].
        let count = usize::from(payload[0]) + 1;
        let body = &payload[1..];
        if count > 0 && body.len() % count == 0 {
            let sz = body.len() / count;
            return (0..count).map(|i| &body[i * sz..(i + 1) * sz]).collect();
        }
    }
    vec![payload]
}

fn scale_to_90k(ticks: u64, timestamp_scale: u64) -> u64 {
    // ticks * timestamp_scale(ns) * 90000 / 1e9
    ((u128::from(ticks) * u128::from(timestamp_scale) * 90_000) / 1_000_000_000) as u64
}

/// Assign each sample a duration from the next sample's PTS; the final sample
/// uses the track's DefaultDuration (or the previous gap).
fn fill_durations(samples: &mut [Sample], default_duration_ns: u64, _scale: u64) {
    let default_ticks = (u128::from(default_duration_ns) * 90_000 / 1_000_000_000) as u32;
    let n = samples.len();
    for i in 0..n {
        let dur = if i + 1 < n {
            samples[i + 1].pts.saturating_sub(samples[i].pts) as u32
        } else if default_ticks > 0 {
            default_ticks
        } else if i > 0 {
            samples[i - 1].duration
        } else {
            1
        };
        samples[i].duration = dur.max(1);
    }
}

/// Build the sample entry + `StreamInfo` for a track from its codec id.
fn build_stream(def: &TrackDef, _samples: &[Sample]) -> Option<(Vec<u8>, StreamInfo)> {
    let (sample_entry, codec, codec_string) = match def.codec_id.as_str() {
        "V_VP9" => {
            let (e, c) = entries::vp9_entry(def.width, def.height);
            (e, Codec::Vp9, Some(c))
        }
        "V_VP8" => {
            let (e, c) = entries::vp8_entry(def.width, def.height);
            (e, Codec::Vp8, Some(c))
        }
        "V_AV1" => {
            let (e, c) = entries::av1_entry(&def.codec_private, def.width, def.height);
            (e, Codec::Av1, Some(c))
        }
        "A_OPUS" => {
            let (e, c) = entries::opus_entry(&def.codec_private, def.channels)?;
            (e, Codec::Opus, Some(c))
        }
        _ => return None, // unsupported codec (Vorbis, etc.)
    };
    let info = StreamInfo {
        kind: def.kind,
        codec,
        timescale: Timescale::MPEG_TS,
        resolution: if def.kind == MediaKind::Video {
            Some((u32::from(def.width), u32::from(def.height)))
        } else {
            None
        },
        sample_rate: if def.kind == MediaKind::Audio { Some(def.sample_rate) } else { None },
        bitrate: None,
        codec_string,
    };
    Some((sample_entry, info))
}

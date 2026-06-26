//! End-to-end MPEG-TS demuxer tests against a hand-built transport stream.

mod common;

use common::{AUDIO_PID, VIDEO_PID, build_ts_aac, build_ts_hevc, build_ts_video};
use sheathe_core::{Codec, MediaKind, SampleFlags};
use sheathe_ts::TsDemuxer;

#[test]
fn demuxes_h264_video_track() {
    let ts = build_ts_video();
    let demux = TsDemuxer::parse(&ts).expect("parse");

    assert_eq!(demux.tracks().len(), 1);
    let track = &demux.tracks()[0];
    assert_eq!(track.pid, VIDEO_PID);
    assert_eq!(track.info.kind, MediaKind::Video);
    assert_eq!(track.info.codec, Codec::H264);
    assert!(track.info.codec_string.as_deref().is_some_and(|s| s.starts_with("avc1.")));
    assert_eq!(track.samples.len(), 1);
    assert!(track.samples[0].flags.contains(SampleFlags::KEYFRAME));
    assert!(!track.sample_entry.is_empty());
}

#[test]
fn demuxes_hevc_video_track() {
    let ts = build_ts_hevc();
    let demux = TsDemuxer::parse(&ts).expect("parse");

    assert_eq!(demux.tracks().len(), 1);
    let track = &demux.tracks()[0];
    assert_eq!(track.pid, VIDEO_PID);
    assert_eq!(track.info.kind, MediaKind::Video);
    assert_eq!(track.info.codec, Codec::H265);
    assert!(track.info.codec_string.as_deref().is_some_and(|s| s.starts_with("hvc1.")));
    assert_eq!(track.samples.len(), 1);
    assert!(track.samples[0].flags.contains(SampleFlags::KEYFRAME));
    assert!(!track.sample_entry.is_empty());
}

#[test]
fn demuxes_adts_aac_track() {
    let ts = build_ts_aac();
    let demux = TsDemuxer::parse(&ts).expect("parse");

    assert_eq!(demux.tracks().len(), 1);
    let track = &demux.tracks()[0];
    assert_eq!(track.pid, AUDIO_PID);
    assert_eq!(track.info.kind, MediaKind::Audio);
    assert_eq!(track.info.codec, Codec::Aac);
    assert_eq!(track.info.sample_rate, Some(44_100));
    assert_eq!(track.samples.len(), 1);
    assert!(!track.sample_entry.is_empty());
}

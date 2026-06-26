//! Raw elementary stream demuxer tests.

use sheathe_core::{Codec, MediaKind, SampleFlags};
use sheathe_es::{EsDemuxer, StreamKind};

fn h264_annex_b() -> Vec<u8> {
    let sps = [0x67, 0x42, 0x00, 0x1e, 0x96, 0x54, 0x05, 0x01, 0xed, 0x80];
    let pps = [0x68, 0xce, 0x3c, 0x80];
    let idr = [0x65, 0x88, 0x84, 0x00, 0x10];
    let mut out = Vec::new();
    for nal in [&sps[..], &pps[..], &idr[..]] {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

fn adts_frame() -> Vec<u8> {
    vec![0xff, 0xf1, 0x50, 0x80, 0x00, 0xe0, 0x00]
}

#[test]
fn demuxes_h264_annex_b() {
    let data = h264_annex_b();
    let demux = EsDemuxer::parse(&data, StreamKind::H264AnnexB).expect("parse");
    let track = demux.track();
    assert_eq!(track.info.kind, MediaKind::Video);
    assert_eq!(track.info.codec, Codec::H264);
    assert_eq!(track.samples.len(), 1);
    assert!(track.samples[0].flags.contains(SampleFlags::KEYFRAME));
}

#[test]
fn demuxes_adts_aac() {
    let data = adts_frame();
    let demux = EsDemuxer::parse_auto("audio.aac", &data).expect("parse");
    let track = demux.track();
    assert_eq!(track.info.kind, MediaKind::Audio);
    assert_eq!(track.info.codec, Codec::Aac);
    assert_eq!(track.info.sample_rate, Some(44_100));
    assert_eq!(track.samples.len(), 1);
}

#[test]
fn detects_by_extension() {
    let data = h264_annex_b();
    let demux = EsDemuxer::parse_auto("clip.h264", &data).expect("parse");
    assert_eq!(demux.track().info.codec, Codec::H264);
}
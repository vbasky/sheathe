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

/// One 128-byte AC-3 syncframe: 48 kHz, frmsizecod 0, bsid 8, acmod 2 (2/0).
fn ac3_frame() -> Vec<u8> {
    let mut f = vec![0x0b, 0x77, 0x00, 0x00, 0x00, 0x40, 0x40];
    f.resize(128, 0);
    f
}

/// One 96-byte E-AC-3 syncframe: 48 kHz, 6 blocks, acmod 7 + LFE (5.1), bsid 16.
fn eac3_frame() -> Vec<u8> {
    let mut f = vec![0x0b, 0x77, 0x00, 0x2f, 0x3f, 0x80];
    f.resize(96, 0);
    f
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
fn demuxes_ac3() {
    let frame = ac3_frame();
    let mut data = frame.clone();
    data.extend_from_slice(&frame); // two syncframes
    let demux = EsDemuxer::parse_auto("audio.ac3", &data).expect("parse");
    let track = demux.track();
    assert_eq!(track.info.kind, MediaKind::Audio);
    assert_eq!(track.info.codec, Codec::Ac3);
    assert_eq!(track.info.sample_rate, Some(48_000));
    assert_eq!(track.info.codec_string.as_deref(), Some("ac-3"));
    assert_eq!(track.samples.len(), 2);
    assert!(track.samples[0].flags.contains(SampleFlags::KEYFRAME));
    // Sample entry is a well-formed `ac-3` AudioSampleEntry.
    assert_eq!(&track.sample_entry[4..8], b"ac-3");
}

#[test]
fn detects_ac3_by_content() {
    let data = ac3_frame();
    // No extension hint → must sniff the 0x0B77 syncword.
    let demux = EsDemuxer::parse_auto("stream.bin", &data).expect("parse");
    assert_eq!(demux.track().info.codec, Codec::Ac3);
}

#[test]
fn demuxes_eac3() {
    let frame = eac3_frame();
    let mut data = frame.clone();
    data.extend_from_slice(&frame);
    let demux = EsDemuxer::parse_auto("audio.eac3", &data).expect("parse");
    let track = demux.track();
    assert_eq!(track.info.kind, MediaKind::Audio);
    assert_eq!(track.info.codec, Codec::Eac3);
    assert_eq!(track.info.sample_rate, Some(48_000));
    assert_eq!(track.info.codec_string.as_deref(), Some("ec-3"));
    assert_eq!(track.samples.len(), 2);
    assert_eq!(&track.sample_entry[4..8], b"ec-3");
}

#[test]
fn distinguishes_ac3_from_eac3_by_content() {
    // Same syncword, routed by bsid at byte 5.
    assert_eq!(
        EsDemuxer::parse_auto("a.bin", &ac3_frame()).unwrap().track().info.codec,
        Codec::Ac3
    );
    assert_eq!(
        EsDemuxer::parse_auto("b.bin", &eac3_frame()).unwrap().track().info.codec,
        Codec::Eac3
    );
}

#[test]
fn detects_by_extension() {
    let data = h264_annex_b();
    let demux = EsDemuxer::parse_auto("clip.h264", &data).expect("parse");
    assert_eq!(demux.track().info.codec, Codec::H264);
}

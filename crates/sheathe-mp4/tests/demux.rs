//! End-to-end demuxer test against a hand-built MP4 (see `common`).

mod common;

use common::{HEIGHT, SAMPLE_DELTA, SIZES, TIMESCALE, WIDTH, build_mp4};
use sheathe_core::{Codec, MediaKind, SampleFlags};
use sheathe_mp4::Mp4Demuxer;

#[test]
fn demuxes_single_video_track() {
    let mp4 = build_mp4();
    let demux = Mp4Demuxer::parse(&mp4).expect("parse");

    assert_eq!(demux.tracks().len(), 1);
    let track = &demux.tracks()[0];
    assert_eq!(track.track_id, 1);
    assert_eq!(track.sample_count, SIZES.len() as u32);

    let info = &track.info;
    assert_eq!(info.kind, MediaKind::Video);
    assert_eq!(info.codec, Codec::H264);
    assert_eq!(info.timescale.0, TIMESCALE);
    assert_eq!(info.resolution, Some((u32::from(WIDTH), u32::from(HEIGHT))));
    assert!(info.bitrate.is_some(), "average bitrate should be computed");
}

#[test]
fn reconstructs_samples_and_timing() {
    let mp4 = build_mp4();
    let demux = Mp4Demuxer::parse(&mp4).expect("parse");
    let samples = demux.samples(0).expect("samples");

    assert_eq!(samples.len(), SIZES.len());

    // Sizes, contents, and decode timing.
    for (i, s) in samples.iter().enumerate() {
        assert_eq!(s.data.len(), SIZES[i] as usize, "sample {i} size");
        assert!(s.data.iter().all(|&b| b == 0x10 + i as u8), "sample {i} bytes");
        assert_eq!(s.dts, i as u64 * u64::from(SAMPLE_DELTA), "sample {i} dts");
        assert_eq!(s.duration, SAMPLE_DELTA);
    }

    // Only the first sample is a keyframe (per stss).
    assert!(samples[0].flags.contains(SampleFlags::KEYFRAME));
    assert!(!samples[1].flags.contains(SampleFlags::KEYFRAME));
    assert!(!samples[2].flags.contains(SampleFlags::KEYFRAME));
    assert!(!samples[3].flags.contains(SampleFlags::KEYFRAME));
}

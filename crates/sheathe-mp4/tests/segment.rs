//! Structural test for the CMAF writer: demux a synthetic MP4, write an init
//! segment and a media segment, then parse the output back and assert the box
//! layout, the `trun` sample count, and the `mdat` payload size.

mod common;

use common::{build_mp4, SIZES};
use sheathe_mp4::{
    fragment, top_level, write_init_segment, write_media_segment, Cursor, Mp4Box, Mp4Demuxer,
    SegmentPolicy,
};

/// Collect the top-level box types of a buffer.
fn top_types(data: &[u8]) -> Vec<[u8; 4]> {
    top_level(data).map(|b| b.expect("box").kind).collect()
}

/// Find a top-level box of a given type.
fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Mp4Box<'a> {
    top_level(data)
        .map(|b| b.expect("box"))
        .find(|b| &b.kind == kind)
        .unwrap_or_else(|| panic!("missing top-level {}", String::from_utf8_lossy(kind)))
}

#[test]
fn init_segment_has_ftyp_moov_mvex() {
    let mp4 = build_mp4();
    let demux = Mp4Demuxer::parse(&mp4).expect("parse");
    let init = write_init_segment(&demux.tracks()[0], None);

    assert_eq!(top_types(&init), vec![*b"ftyp", *b"moov"]);

    let moov = find(&init, b"moov");
    let trak = moov.child(b"trak").expect("ok").expect("trak");
    assert!(
        moov.child(b"mvex").expect("ok").is_some(),
        "moov needs mvex"
    );

    // The avc1 sample entry is carried through into stsd.
    let stbl = trak
        .child(b"mdia")
        .unwrap()
        .unwrap()
        .child(b"minf")
        .unwrap()
        .unwrap()
        .child(b"stbl")
        .unwrap()
        .unwrap();
    let stsd = stbl.child(b"stsd").unwrap().unwrap();
    assert!(
        stsd.body.windows(4).any(|w| w == b"avc1"),
        "stsd should carry the avc1 sample entry"
    );
}

#[test]
fn media_segment_layout_and_sizes() {
    let mp4 = build_mp4();
    let demux = Mp4Demuxer::parse(&mp4).expect("parse");
    let track = &demux.tracks()[0];
    let samples = demux.samples(0).expect("samples");

    // All four samples land in a single segment under the default policy.
    let segments = fragment(&track.info, samples, SegmentPolicy::default()).expect("fragment");
    assert_eq!(segments.len(), 1);
    let seg = &segments[0];

    let media = write_media_segment(track, 1, seg, 0, None);
    assert_eq!(
        top_types(&media),
        vec![*b"styp", *b"sidx", *b"moof", *b"mdat"]
    );

    // trun sample_count matches the segment.
    let moof = find(&media, b"moof");
    let traf = moof.child(b"traf").unwrap().unwrap();
    let trun = traf.child(b"trun").unwrap().unwrap();
    let mut c = Cursor::new(trun.body);
    c.version_flags().unwrap();
    let sample_count = c.u32().unwrap();
    assert_eq!(sample_count as usize, seg.samples.len());

    // mdat payload equals the sum of sample sizes.
    let mdat = find(&media, b"mdat");
    assert_eq!(mdat.body.len() as u32, SIZES.iter().sum::<u32>());
}

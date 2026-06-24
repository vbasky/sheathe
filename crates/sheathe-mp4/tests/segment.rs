//! Structural test for the CMAF writer: demux a synthetic MP4, write an init
//! segment and a media segment, then parse the output back and assert the box
//! layout, the `trun` sample count, and the `mdat` payload size.

mod common;

use common::{SIZES, build_mp4};
use sheathe_mp4::{
    Cursor, Mp4Box, Mp4Demuxer, SegmentPolicy, fragment, top_level, write_init_segment,
    write_media_segment,
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
    assert!(moov.child(b"mvex").expect("ok").is_some(), "moov needs mvex");

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
    assert!(stsd.body.windows(4).any(|w| w == b"avc1"), "stsd should carry the avc1 sample entry");
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
    assert_eq!(top_types(&media), vec![*b"styp", *b"sidx", *b"moof", *b"mdat"]);

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

/// First byte index of `needle` in `data`.
fn index_of(data: &[u8], needle: &[u8]) -> usize {
    data.windows(needle.len()).position(|w| w == needle).expect("needle present")
}

#[test]
fn encrypted_segment_has_cenc_boxes_and_aux_offset() {
    use sheathe_crypto::{ContentKey, ProtectionSystem, Scheme};
    use sheathe_mp4::Encryption;

    let mp4 = build_mp4();
    let demux = Mp4Demuxer::parse(&mp4).expect("parse");
    let track = &demux.tracks()[0];
    let samples = demux.samples(0).expect("samples");
    let segments = fragment(&track.info, samples, SegmentPolicy::default()).expect("fragment");

    let enc = Encryption {
        scheme: Scheme::Cenc,
        key: ContentKey { kid: [0x11; 16], key: [0x22; 16] },
        constant_iv: [0; 16],
        systems: vec![ProtectionSystem::Common],
        crypto_period_seconds: None,
    };

    // Init switches the sample entry to encv and carries tenc + pssh.
    let init = write_init_segment(track, Some(&enc));
    for tag in [b"encv", b"tenc", b"pssh"] {
        assert!(init.windows(4).any(|w| w == tag), "init missing {}", String::from_utf8_lossy(tag));
    }

    // Media segment carries saiz + saio + senc in the traf.
    let media = write_media_segment(track, 1, &segments[0], 0, Some(&enc));
    let traf = find(&media, b"moof").child(b"traf").unwrap().unwrap();
    let saio = traf.child(b"saio").unwrap().expect("saio");
    assert!(traf.child(b"saiz").unwrap().is_some(), "saiz present");
    assert!(traf.child(b"senc").unwrap().is_some(), "senc present");

    // saio offset is moof-relative and points at the senc per-sample aux data
    // (12 bytes past the `senc` type: version/flags + sample_count).
    let off = u32::from_be_bytes(saio.body[8..12].try_into().unwrap()) as usize;
    let moof_start = index_of(&media, b"moof") - 4; // box size precedes the type
    let senc_aux = index_of(&media, b"senc") + 12;
    assert_eq!(moof_start + off, senc_aux, "saio must point at senc aux data");
}

#[test]
fn key_rotation_emits_seig_groups_and_zero_tenc_kid() {
    use sheathe_crypto::{ContentKey, ProtectionSystem, Scheme};
    use sheathe_mp4::Encryption;

    let mp4 = build_mp4();
    let demux = Mp4Demuxer::parse(&mp4).expect("parse");
    let track = &demux.tracks()[0];
    let samples = demux.samples(0).expect("samples");
    let segments = fragment(&track.info, samples, SegmentPolicy::default()).expect("fragment");

    let enc = Encryption {
        scheme: Scheme::Cenc,
        key: ContentKey { kid: [0x11; 16], key: [0x22; 16] },
        constant_iv: [0; 16],
        systems: vec![ProtectionSystem::Widevine],
        crypto_period_seconds: Some(0.0001), // tiny period so rotation is active
    };

    // Rotation moves the pssh out of the init; the tenc default KID is zeroed.
    let init = write_init_segment(track, Some(&enc));
    assert!(!init.windows(4).any(|w| w == b"pssh"), "init must not carry pssh when rotating");
    let tenc = index_of(&init, b"tenc");
    let kid = &init[tenc + 4 + 4 + 4..tenc + 4 + 4 + 4 + 16]; // type+ver/flags+(rsv,rsv,prot,iv)
    assert_eq!(kid, &[0u8; 16], "rotating tenc default KID is zero");

    // The media segment carries the seig sample group and a per-period pssh.
    let media = write_media_segment(track, 1, &segments[0], 0, Some(&enc));
    let traf = find(&media, b"moof").child(b"traf").unwrap().unwrap();
    assert!(traf.child(b"sbgp").unwrap().is_some(), "sbgp present");
    assert!(traf.child(b"sgpd").unwrap().is_some(), "sgpd present");
    assert!(media.windows(4).any(|w| w == b"pssh"), "per-segment pssh present");
}

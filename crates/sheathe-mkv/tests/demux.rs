//! WebM/Matroska demuxer integration tests (hand-encoded EBML).

use sheathe_core::{Codec, MediaKind, SampleFlags};
use sheathe_mkv::MkvDemuxer;

/// Encode one EBML element: raw `id` bytes + var-int size + `body`.
fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.extend_from_slice(&vint(body.len() as u64));
    out.extend_from_slice(body);
    out
}

/// Encode a size as an EBML var-int (1 or 2 bytes here).
fn vint(n: u64) -> Vec<u8> {
    if n < 0x7f { vec![0x80 | n as u8] } else { vec![0x40 | (n >> 8) as u8, (n & 0xff) as u8] }
}

fn opus_head() -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1); // version
    h.push(2); // channels
    h.extend_from_slice(&312u16.to_le_bytes()); // pre-skip
    h.extend_from_slice(&48_000u32.to_le_bytes()); // input sample rate
    h.extend_from_slice(&0i16.to_le_bytes()); // output gain
    h.push(0); // mapping family
    h
}

fn simple_block(track: u8, rel_ts: i16, keyframe: bool, payload: &[u8]) -> Vec<u8> {
    let mut b = vec![0x80 | track]; // track number var-int
    b.extend_from_slice(&rel_ts.to_be_bytes());
    b.push(if keyframe { 0x80 } else { 0x00 });
    b.extend_from_slice(payload);
    b
}

fn webm_opus() -> Vec<u8> {
    // Audio TrackEntry (track 1, A_OPUS, 2ch, 48k).
    let audio = el(&[0x9f], &[0x02]); // Channels = 2
    let freq = el(&[0xb5], &48_000.0f32.to_be_bytes()); // SamplingFrequency
    let audio_master = {
        let mut m = audio;
        m.extend_from_slice(&freq);
        el(&[0xe1], &m)
    };
    let mut track_entry = Vec::new();
    track_entry.extend_from_slice(&el(&[0xd7], &[0x01])); // TrackNumber
    track_entry.extend_from_slice(&el(&[0x83], &[0x02])); // TrackType = audio
    track_entry.extend_from_slice(&el(&[0x86], b"A_OPUS")); // CodecID
    track_entry.extend_from_slice(&el(&[0x63, 0xa2], &opus_head())); // CodecPrivate
    track_entry.extend_from_slice(&audio_master);
    let tracks = el(&[0x16, 0x54, 0xae, 0x6b], &el(&[0xae], &track_entry));

    // Info: TimestampScale = 1_000_000 (1 ms).
    let info = el(&[0x15, 0x49, 0xa9, 0x66], &el(&[0x2a, 0xd7, 0xb1], &1_000_000u32.to_be_bytes()));

    // Cluster: base ts 0, two SimpleBlocks 20 ms apart.
    let mut cluster = el(&[0xe7], &[0x00]); // Timestamp = 0
    cluster.extend_from_slice(&el(&[0xa3], &simple_block(1, 0, true, &[1, 2, 3, 4])));
    cluster.extend_from_slice(&el(&[0xa3], &simple_block(1, 20, true, &[5, 6, 7, 8])));
    let cluster = el(&[0x1f, 0x43, 0xb6, 0x75], &cluster);

    let mut segment_body = Vec::new();
    segment_body.extend_from_slice(&info);
    segment_body.extend_from_slice(&tracks);
    segment_body.extend_from_slice(&cluster);
    let segment = el(&[0x18, 0x53, 0x80, 0x67], &segment_body);

    // EBML header (content irrelevant to our parser) + Segment.
    let mut out = el(&[0x1a, 0x45, 0xdf, 0xa3], &[0x00]);
    out.extend_from_slice(&segment);
    out
}

#[test]
fn demuxes_opus_track() {
    let data = webm_opus();
    let demux = MkvDemuxer::parse(&data).expect("parse");
    assert_eq!(demux.tracks().len(), 1);
    let t = &demux.tracks()[0];
    assert_eq!(t.info.kind, MediaKind::Audio);
    assert_eq!(t.info.codec, Codec::Opus);
    assert_eq!(t.info.sample_rate, Some(48_000));
    assert_eq!(t.info.codec_string.as_deref(), Some("Opus"));
    assert_eq!(t.samples.len(), 2);
    assert!(t.samples[0].flags.contains(SampleFlags::KEYFRAME));
    // 20 ms @ 90 kHz = 1800 ticks.
    assert_eq!(t.samples[1].pts, 1_800);
    assert_eq!(t.samples[0].duration, 1_800);
    // Sample entry is a well-formed `Opus` box with a `dOps`.
    assert_eq!(&t.sample_entry[4..8], b"Opus");
    assert!(t.sample_entry.windows(4).any(|w| w == b"dOps"));
}

#[test]
fn rejects_non_webm() {
    assert!(MkvDemuxer::parse(&[0, 0, 0, 0]).is_err());
}

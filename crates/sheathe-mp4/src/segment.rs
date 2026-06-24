//! CMAF / fragmented-MP4 writer: init segments and media segments.
//!
//! This is the write side that makes `package` produce playable output — Shaka
//! Packager's MP4 muxer. One **init segment** (`ftyp` + `moov` + `mvex`) and a
//! sequence of **media segments** (`styp` + `sidx` + `moof` + `mdat`) per track,
//! the CMAF layout DASH and HLS reference.
//!
//! Codec configuration (`avcC`/`esds`) is re-emitted verbatim from the input's
//! sample entry (see [`Track::sample_entry`]), so output is playable without
//! rebuilding the decoder config.

use crate::box_writer::{BoxWriter, FourCc};
use crate::demux::Track;
use crate::fragmenter::Segment;
use sheathe_core::{Codec, MediaKind, SampleFlags};
use sheathe_crypto::{ContentKey, Encryptor, Scheme, Subsample};

/// CENC encryption parameters for a packaging run.
pub struct Encryption {
    /// Protection scheme (`cenc` = per-sample IV CTR; `cbcs` = constant-IV CBC pattern).
    pub scheme: Scheme,
    /// The content key + KID.
    pub key: ContentKey,
    /// Constant IV used by `cbcs` (stored in `tenc`, reused for every sample).
    /// Ignored by `cenc`, which derives a unique per-sample IV.
    pub constant_iv: [u8; 16],
}

impl Encryption {
    /// Per-sample IV size written in `tenc`/`senc`: 16 for `cenc`, 0 for `cbcs`
    /// (which signals a constant IV).
    fn per_sample_iv_size(&self) -> u8 {
        match self.scheme {
            Scheme::Cenc => 16,
            Scheme::Cbcs => 0,
        }
    }
}

/// Common PSSH SystemID (`urn:mpeg:dash:mp4protection` / W3C clear-key family).
const COMMON_SYSTEM_ID: [u8; 16] = [
    0x10, 0x77, 0xef, 0xec, 0xc0, 0xb2, 0x4d, 0x02, 0xac, 0xe3, 0x3c, 0x1e, 0x52, 0xe2, 0xfb, 0x4b,
];

/// Per-sample encryption result: IV, subsample layout, and encrypted bytes.
struct SampleEnc {
    iv: [u8; 16],
    subsamples: Vec<Subsample>,
    data: Vec<u8>,
}

/// Movie timescale used in the init segment's `mvhd`.
const MOVIE_TIMESCALE: u32 = 1000;

/// Unity 3x3 transformation matrix (16.16 / 2.30 fixed point).
const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn fcc(s: &[u8; 4]) -> FourCc {
    FourCc(*s)
}

/// Write a full-box version/flags word.
fn full(w: &mut BoxWriter, version: u8, flags: u32) {
    w.u32((u32::from(version) << 24) | (flags & 0x00ff_ffff));
}

/// Build the CMAF initialization segment (`ftyp` + `moov`) for one track.
///
/// When `enc` is set, the sample entry is wrapped as `encv`/`enca` with a `sinf`
/// (`frma`/`schm`/`tenc`) and a `pssh` is added to `moov`.
pub fn write_init_segment(track: &Track, enc: Option<&Encryption>) -> Vec<u8> {
    let mut w = BoxWriter::new();
    write_ftyp(&mut w);
    write_moov(&mut w, track, enc);
    w.into_bytes()
}

/// Build one media segment (`styp` + `sidx` + `moof` + `mdat`) for `track`.
///
/// `sequence_number` is the 1-based `mfhd` sequence number. `first_sample_index`
/// is the global index (within the track) of this segment's first sample, used
/// to derive unique per-sample IVs. When `enc` is set, sample data is encrypted
/// and a `senc` box is emitted.
pub fn write_media_segment(
    track: &Track,
    sequence_number: u32,
    segment: &Segment,
    first_sample_index: u64,
    enc: Option<&Encryption>,
) -> Vec<u8> {
    let enc_samples = enc.map(|e| encrypt_segment(track, segment, first_sample_index, e));
    let iv_size = enc.map_or(0, Encryption::per_sample_iv_size);
    let moof = build_moof(track, sequence_number, segment, enc_samples.as_deref(), iv_size);
    let mdat = build_mdat(segment, enc_samples.as_deref());
    let referenced_size = (moof.len() + mdat.len()) as u32;
    let sidx = build_sidx(track, segment, referenced_size);
    let styp = build_styp();
    [styp, sidx, moof, mdat].concat()
}

fn write_ftyp(w: &mut BoxWriter) {
    w.begin(fcc(b"ftyp"));
    w.bytes(b"iso6"); // major brand
    w.u32(0); // minor version
    for brand in [b"iso6", b"cmfc", b"dash", b"mp41"] {
        w.bytes(brand);
    }
    w.end();
}

fn write_moov(w: &mut BoxWriter, track: &Track, enc: Option<&Encryption>) {
    w.begin(fcc(b"moov"));

    // mvhd (version 0)
    w.begin(fcc(b"mvhd"));
    full(w, 0, 0);
    w.u32(0); // creation
    w.u32(0); // modification
    w.u32(MOVIE_TIMESCALE);
    w.u32(0); // duration (0 = unknown, fragmented)
    w.u32(0x0001_0000); // rate 1.0
    w.u16(0x0100); // volume 1.0
    w.u16(0); // reserved
    w.u32(0);
    w.u32(0); // reserved[2]
    for m in UNITY_MATRIX {
        w.u32(m);
    }
    for _ in 0..6 {
        w.u32(0); // pre_defined
    }
    w.u32(track.track_id + 1); // next_track_id
    w.end();

    write_trak(w, track, enc);

    if let Some(e) = enc {
        write_pssh(w, e);
    }

    // mvex > trex
    w.begin(fcc(b"mvex"));
    w.begin(fcc(b"trex"));
    full(w, 0, 0);
    w.u32(track.track_id);
    w.u32(1); // default_sample_description_index
    w.u32(0); // default_sample_duration
    w.u32(0); // default_sample_size
    w.u32(0); // default_sample_flags
    w.end();
    w.end();

    w.end(); // moov
}

fn write_trak(w: &mut BoxWriter, track: &Track, enc: Option<&Encryption>) {
    w.begin(fcc(b"trak"));

    // tkhd (version 0): enabled | in-movie | in-preview
    w.begin(fcc(b"tkhd"));
    full(w, 0, 0x0000_0007);
    w.u32(0); // creation
    w.u32(0); // modification
    w.u32(track.track_id);
    w.u32(0); // reserved
    w.u32(0); // duration
    w.u32(0);
    w.u32(0); // reserved[2]
    w.u16(0); // layer
    w.u16(0); // alternate_group
    w.u16(if track.info.kind == MediaKind::Audio { 0x0100 } else { 0 }); // volume
    w.u16(0); // reserved
    for m in UNITY_MATRIX {
        w.u32(m);
    }
    let (width, height) = track.info.resolution.unwrap_or((0, 0));
    w.u32(width << 16); // width 16.16
    w.u32(height << 16); // height 16.16
    w.end();

    // mdia
    w.begin(fcc(b"mdia"));

    // mdhd (version 0)
    w.begin(fcc(b"mdhd"));
    full(w, 0, 0);
    w.u32(0); // creation
    w.u32(0); // modification
    w.u32(track.info.timescale.0);
    w.u32(0); // duration
    w.u16(0x55c4); // language 'und'
    w.u16(0); // pre_defined
    w.end();

    // hdlr
    let (handler, name): (&[u8; 4], &str) = match track.info.kind {
        MediaKind::Video => (b"vide", "VideoHandler"),
        MediaKind::Audio => (b"soun", "SoundHandler"),
        MediaKind::Text => (b"text", "TextHandler"),
    };
    w.begin(fcc(b"hdlr"));
    full(w, 0, 0);
    w.u32(0); // pre_defined
    w.bytes(handler); // handler_type
    w.u32(0);
    w.u32(0);
    w.u32(0); // reserved[3]
    w.bytes(name.as_bytes());
    w.u8(0); // null terminator
    w.end();

    // minf
    w.begin(fcc(b"minf"));
    match track.info.kind {
        MediaKind::Video => {
            w.begin(fcc(b"vmhd"));
            full(w, 0, 1);
            w.u16(0); // graphicsmode
            w.u16(0);
            w.u16(0);
            w.u16(0); // opcolor
            w.end();
        }
        MediaKind::Audio => {
            w.begin(fcc(b"smhd"));
            full(w, 0, 0);
            w.u16(0); // balance
            w.u16(0); // reserved
            w.end();
        }
        MediaKind::Text => {
            w.begin(fcc(b"nmhd"));
            full(w, 0, 0);
            w.end();
        }
    }

    // dinf > dref > url (self-contained)
    w.begin(fcc(b"dinf"));
    w.begin(fcc(b"dref"));
    full(w, 0, 0);
    w.u32(1); // entry_count
    w.begin(fcc(b"url "));
    full(w, 0, 1); // self-contained
    w.end();
    w.end();
    w.end();

    // stbl with empty sample tables (samples live in media segments)
    w.begin(fcc(b"stbl"));
    w.begin(fcc(b"stsd"));
    full(w, 0, 0);
    w.u32(1); // entry_count
    match enc {
        Some(e) => w.bytes(&protected_sample_entry(track, e)),
        None => w.bytes(track.sample_entry()),
    }
    w.end();
    for empty in [b"stts", b"stsc"] {
        w.begin(fcc(empty));
        full(w, 0, 0);
        w.u32(0); // entry_count
        w.end();
    }
    w.begin(fcc(b"stsz"));
    full(w, 0, 0);
    w.u32(0); // sample_size
    w.u32(0); // sample_count
    w.end();
    w.begin(fcc(b"stco"));
    full(w, 0, 0);
    w.u32(0); // entry_count
    w.end();
    w.end(); // stbl

    w.end(); // minf
    w.end(); // mdia
    w.end(); // trak
}

fn build_styp() -> Vec<u8> {
    let mut w = BoxWriter::new();
    w.begin(fcc(b"styp"));
    w.bytes(b"msdh"); // major brand
    w.u32(0); // minor version
    for brand in [b"msdh", b"msix"] {
        w.bytes(brand);
    }
    w.end();
    w.into_bytes()
}

fn build_moof(
    track: &Track,
    sequence_number: u32,
    segment: &Segment,
    enc_samples: Option<&[SampleEnc]>,
    per_sample_iv_size: u8,
) -> Vec<u8> {
    // Composition offsets are only needed when any sample is reordered (B-frames).
    let any_cts = segment.samples.iter().any(|s| s.pts != s.dts);

    let mut w = BoxWriter::new();
    w.begin(fcc(b"moof"));

    // mfhd
    w.begin(fcc(b"mfhd"));
    full(&mut w, 0, 0);
    w.u32(sequence_number);
    w.end();

    // traf
    w.begin(fcc(b"traf"));

    // tfhd: default-base-is-moof (0x020000) so trun data offsets are moof-relative.
    w.begin(fcc(b"tfhd"));
    full(&mut w, 0, 0x02_0000);
    w.u32(track.track_id);
    w.end();

    // tfdt (version 1): base media decode time = first sample dts of the segment.
    w.begin(fcc(b"tfdt"));
    full(&mut w, 1, 0);
    w.u64(segment.start_ticks);
    w.end();

    // trun: data-offset + per-sample duration/size/flags (+ composition offset).
    let mut tr_flags = 0x0001 | 0x0100 | 0x0200 | 0x0400;
    if any_cts {
        tr_flags |= 0x0800;
    }
    let version = u8::from(any_cts);
    w.begin(fcc(b"trun"));
    full(&mut w, version, tr_flags);
    w.u32(segment.samples.len() as u32);
    let data_offset_pos = w.pos(); // backpatched once the moof length is known
    w.u32(0); // data_offset placeholder
    for s in &segment.samples {
        w.u32(s.duration);
        w.u32(s.data.len() as u32);
        w.u32(sample_flags(s.flags));
        if any_cts {
            let cts = (s.pts as i64 - s.dts as i64) as i32;
            w.u32(cts as u32);
        }
    }
    w.end(); // trun

    if let Some(samples) = enc_samples {
        write_senc(&mut w, samples, per_sample_iv_size);
    }

    w.end(); // traf
    w.end(); // moof

    let mut bytes = w.into_bytes();
    // data_offset is moof-relative: start of mdat payload = moof length + mdat header (8).
    let data_offset = (bytes.len() + 8) as u32;
    bytes[data_offset_pos..data_offset_pos + 4].copy_from_slice(&data_offset.to_be_bytes());
    bytes
}

fn build_mdat(segment: &Segment, enc_samples: Option<&[SampleEnc]>) -> Vec<u8> {
    let mut w = BoxWriter::new();
    w.begin(fcc(b"mdat"));
    if let Some(es) = enc_samples {
        for e in es {
            w.bytes(&e.data);
        }
    } else {
        for s in &segment.samples {
            w.bytes(&s.data);
        }
    }
    w.end();
    w.into_bytes()
}

fn build_sidx(track: &Track, segment: &Segment, referenced_size: u32) -> Vec<u8> {
    let earliest_pts = segment.samples.first().map_or(segment.start_ticks, |s| s.pts);
    let mut w = BoxWriter::new();
    w.begin(fcc(b"sidx"));
    full(&mut w, 1, 0);
    w.u32(track.track_id); // reference_id
    w.u32(track.info.timescale.0); // timescale
    w.u64(earliest_pts); // earliest_presentation_time
    w.u64(0); // first_offset
    w.u16(0); // reserved
    w.u16(1); // reference_count
    // reference_type (0 = media) << 31 | referenced_size
    w.u32(referenced_size & 0x7fff_ffff);
    w.u32(segment.duration_ticks as u32); // subsegment_duration
    // starts_with_SAP (1) << 31 | SAP_type (1) << 28 | SAP_delta (0)
    w.u32(0x9000_0000);
    w.end();
    w.into_bytes()
}

/// Encode `trun` per-sample flags from a sample's keyframe state.
fn sample_flags(flags: SampleFlags) -> u32 {
    if flags.contains(SampleFlags::KEYFRAME) {
        // sample_depends_on = 2 (I-frame), sample_is_non_sync_sample = 0
        0x0200_0000
    } else {
        // sample_depends_on = 1 (P/B), sample_is_non_sync_sample = 1
        0x0101_0000
    }
}

// ---------------------------------------------------------------------------
// CENC encryption
// ---------------------------------------------------------------------------

/// Wrap a track's sample entry as `encv`/`enca` with a `sinf` describing the
/// CENC protection. The original sample entry (with its `avcC`/`esds`) is kept;
/// only its type changes and a `sinf` is appended.
fn protected_sample_entry(track: &Track, enc: &Encryption) -> Vec<u8> {
    let raw = track.sample_entry();
    // raw = [size(4)][fourcc(4)][body…]
    if raw.len() < 8 {
        return raw.to_vec();
    }
    let orig_fourcc: [u8; 4] = raw[4..8].try_into().unwrap();
    let body = &raw[8..];
    let new_type = match track.info.kind {
        MediaKind::Video => b"encv",
        MediaKind::Audio => b"enca",
        MediaKind::Text => return raw.to_vec(),
    };

    let mut w = BoxWriter::new();
    w.begin(fcc(new_type));
    w.bytes(body); // original sample-entry body incl. avcC/esds
    // sinf
    w.begin(fcc(b"sinf"));
    w.begin(fcc(b"frma"));
    w.bytes(&orig_fourcc);
    w.end();
    w.begin(fcc(b"schm"));
    full(&mut w, 0, 0);
    w.bytes(&enc.scheme.scheme_type());
    w.u32(0x0001_0000); // scheme_version
    w.end();
    w.begin(fcc(b"schi"));
    write_tenc(&mut w, enc);
    w.end(); // schi
    w.end(); // sinf
    w.end(); // encv/enca
    w.into_bytes()
}

/// The `tenc` track encryption box. `cenc` uses version 0 with a 16-byte
/// per-sample IV; `cbcs` uses version 1 with the 1:9 pattern and a constant IV.
fn write_tenc(w: &mut BoxWriter, enc: &Encryption) {
    w.begin(fcc(b"tenc"));
    match enc.scheme {
        Scheme::Cenc => {
            full(w, 0, 0);
            w.u8(0); // reserved
            w.u8(0); // reserved (version 0)
            w.u8(1); // default_isProtected
            w.u8(16); // default_Per_Sample_IV_Size
            w.bytes(&enc.key.kid);
        }
        Scheme::Cbcs => {
            full(w, 1, 0);
            w.u8(0); // reserved
            w.u8((1 << 4) | 9); // default_crypt_byte_block=1, default_skip_byte_block=9
            w.u8(1); // default_isProtected
            w.u8(0); // default_Per_Sample_IV_Size = 0 -> constant IV follows
            w.bytes(&enc.key.kid);
            w.u8(16); // default_constant_IV_size
            w.bytes(&enc.constant_iv);
        }
    }
    w.end();
}

/// A `pssh` for the Common (clear-key family) system, listing the KID.
fn write_pssh(w: &mut BoxWriter, enc: &Encryption) {
    w.begin(fcc(b"pssh"));
    full(w, 1, 0); // version 1 carries a KID list
    w.bytes(&COMMON_SYSTEM_ID);
    w.u32(1); // KID_count
    w.bytes(&enc.key.kid);
    w.u32(0); // DataSize
    w.end();
}

/// Encrypt every sample of a segment, returning IV + subsamples + ciphertext.
fn encrypt_segment(
    track: &Track,
    segment: &Segment,
    first_sample_index: u64,
    enc: &Encryption,
) -> Vec<SampleEnc> {
    let encryptor = Encryptor::new(&enc.key.key);
    let nal_len =
        crate::codecs::nal_unit_length_size(&sample_entry_fourcc(track), sample_entry_body(track))
            .unwrap_or(4);
    let header = match track.info.codec {
        Codec::H264 => 1,
        Codec::H265 => 2,
        _ => 0,
    };
    let video = track.info.kind == MediaKind::Video && header > 0;

    segment
        .samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut data = s.data.clone();
            let iv = match enc.scheme {
                Scheme::Cenc => make_iv(track.track_id, first_sample_index + i as u64),
                Scheme::Cbcs => enc.constant_iv,
            };
            let subsamples = if video {
                video_subsamples(&data, nal_len, header)
            } else {
                vec![Subsample { clear: 0, protected: data.len() as u32 }]
            };
            // Encryption never changes length; ignore the validated result.
            let _ = encryptor.encrypt(enc.scheme, &iv, &mut data, &subsamples);
            SampleEnc { iv, subsamples, data }
        })
        .collect()
}

/// Split a length-prefixed AVC/HEVC sample into per-NAL clear/protected runs.
/// Each NAL keeps its length prefix + header byte(s) clear; the payload is
/// protected.
fn video_subsamples(data: &[u8], nal_len: u8, header: usize) -> Vec<Subsample> {
    let nl = nal_len as usize;
    let mut subs = Vec::new();
    let mut p = 0usize;
    while p + nl <= data.len() {
        let mut size = 0usize;
        for &b in &data[p..p + nl] {
            size = (size << 8) | usize::from(b);
        }
        let nal_start = p + nl;
        let nal_end = (nal_start + size).min(data.len());
        let nal_payload = nal_end - nal_start;
        let clear_in_nal = header.min(nal_payload);
        subs.push(Subsample {
            clear: (nl + clear_in_nal) as u32,
            protected: (nal_payload - clear_in_nal) as u32,
        });
        p = nal_end;
    }
    if p < data.len() {
        subs.push(Subsample { clear: (data.len() - p) as u32, protected: 0 });
    }
    subs
}

/// Per-sample IV: track_id in the high 4 bytes, a global sample counter in the
/// low 8 — unique per (track, sample) under one key.
fn make_iv(track_id: u32, counter: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[0..4].copy_from_slice(&track_id.to_be_bytes());
    iv[8..16].copy_from_slice(&counter.to_be_bytes());
    iv
}

/// The `senc` box: optional per-sample IV (omitted when `iv_size` is 0, i.e.
/// `cbcs` with a constant IV) + subsample clear/protected runs.
fn write_senc(w: &mut BoxWriter, samples: &[SampleEnc], iv_size: u8) {
    w.begin(fcc(b"senc"));
    full(w, 0, 0x00_0002); // flags: use_subsamples
    w.u32(samples.len() as u32);
    for s in samples {
        if iv_size > 0 {
            w.bytes(&s.iv[..iv_size as usize]);
        }
        w.u16(s.subsamples.len() as u16);
        for ss in &s.subsamples {
            w.u16(ss.clear as u16);
            w.u32(ss.protected);
        }
    }
    w.end();
}

fn sample_entry_fourcc(track: &Track) -> [u8; 4] {
    let raw = track.sample_entry();
    raw.get(4..8).and_then(|s| s.try_into().ok()).unwrap_or(*b"\0\0\0\0")
}

fn sample_entry_body(track: &Track) -> &[u8] {
    track.sample_entry().get(8..).unwrap_or(&[])
}

//! Shared test fixture: a hand-built, minimal-but-valid MP4.
//!
//! Synthesizing the bytes (rather than committing a binary) keeps CI hermetic
//! and exercises `BoxWriter` on the way in.

use sheathe_mp4::{BoxWriter, FourCc};

pub const TIMESCALE: u32 = 12_800;
pub const SAMPLE_DELTA: u32 = 512; // 25 fps at 12800
pub const SIZES: [u32; 4] = [100, 50, 60, 70];
pub const WIDTH: u16 = 640;
pub const HEIGHT: u16 = 360;

fn fourcc(s: &[u8; 4]) -> FourCc {
    FourCc(*s)
}

/// Build `[ftyp][mdat][moov]` for a single AVC video track with 4 samples,
/// the first of which is a sync sample.
pub fn build_mp4() -> Vec<u8> {
    // ftyp ---------------------------------------------------------------
    let mut ftyp = BoxWriter::new();
    ftyp.begin(fourcc(b"ftyp"));
    ftyp.bytes(b"isom"); // major brand
    ftyp.u32(0); // minor version
    ftyp.bytes(b"isom");
    ftyp.bytes(b"avc1");
    ftyp.end();
    let ftyp = ftyp.into_bytes();

    // mdat data offset = after ftyp + mdat's own 8-byte header.
    let data_offset = (ftyp.len() + 8) as u32;

    // mdat ---------------------------------------------------------------
    let mut mdat = BoxWriter::new();
    mdat.begin(fourcc(b"mdat"));
    for (i, &sz) in SIZES.iter().enumerate() {
        mdat.bytes(&vec![0x10 + i as u8; sz as usize]);
    }
    mdat.end();
    let mdat = mdat.into_bytes();

    // moov ---------------------------------------------------------------
    let mut w = BoxWriter::new();
    w.begin(fourcc(b"moov"));

    // mvhd (minimal, version 0)
    w.begin(fourcc(b"mvhd"));
    w.u32(0); // version + flags
    w.u32(0); // creation
    w.u32(0); // modification
    w.u32(1000); // timescale
    w.u32(0); // duration
    w.u32(0x0001_0000); // rate
    w.u16(0x0100); // volume
    w.u16(0); // reserved
    w.u32(0);
    w.u32(0); // reserved[2]
    for _ in 0..9 {
        w.u32(0); // matrix
    }
    for _ in 0..6 {
        w.u32(0); // pre_defined
    }
    w.u32(2); // next_track_id
    w.end();

    // trak
    w.begin(fourcc(b"trak"));

    // tkhd (version 0)
    w.begin(fourcc(b"tkhd"));
    w.u32(0x0000_0007); // version 0, flags = enabled|in-movie|in-preview
    w.u32(0); // creation
    w.u32(0); // modification
    w.u32(1); // track_id
    w.u32(0); // reserved
    w.u32(0); // duration
    w.u32(0);
    w.u32(0); // reserved[2]
    w.u16(0); // layer
    w.u16(0); // alternate_group
    w.u16(0); // volume
    w.u16(0); // reserved
    for _ in 0..9 {
        w.u32(0); // matrix
    }
    w.u32(u32::from(WIDTH) << 16); // width 16.16
    w.u32(u32::from(HEIGHT) << 16); // height 16.16
    w.end();

    // mdia
    w.begin(fourcc(b"mdia"));

    // mdhd (version 0)
    w.begin(fourcc(b"mdhd"));
    w.u32(0); // version + flags
    w.u32(0); // creation
    w.u32(0); // modification
    w.u32(TIMESCALE);
    w.u32(SAMPLE_DELTA * SIZES.len() as u32); // duration
    w.u16(0x55c4); // language 'und'
    w.u16(0); // pre_defined
    w.end();

    // hdlr
    w.begin(fourcc(b"hdlr"));
    w.u32(0); // version + flags
    w.u32(0); // pre_defined
    w.bytes(b"vide"); // handler_type
    w.u32(0);
    w.u32(0);
    w.u32(0); // reserved[3]
    w.bytes(b"video\0"); // name
    w.end();

    // minf
    w.begin(fourcc(b"minf"));

    // vmhd
    w.begin(fourcc(b"vmhd"));
    w.u32(0x0000_0001); // version 0, flags = 1
    w.u16(0); // graphicsmode
    w.u16(0);
    w.u16(0);
    w.u16(0); // opcolor
    w.end();

    // dinf > dref > url
    w.begin(fourcc(b"dinf"));
    w.begin(fourcc(b"dref"));
    w.u32(0); // version + flags
    w.u32(1); // entry_count
    w.begin(fourcc(b"url "));
    w.u32(0x0000_0001); // self-contained flag
    w.end();
    w.end();
    w.end();

    // stbl
    w.begin(fourcc(b"stbl"));

    // stsd > avc1 (VisualSampleEntry)
    w.begin(fourcc(b"stsd"));
    w.u32(0); // version + flags
    w.u32(1); // entry_count
    w.begin(fourcc(b"avc1"));
    w.bytes(&[0; 6]); // SampleEntry reserved
    w.u16(1); // data_reference_index
    w.u16(0); // pre_defined
    w.u16(0); // reserved
    w.u32(0);
    w.u32(0);
    w.u32(0); // pre_defined[3]
    w.u16(WIDTH);
    w.u16(HEIGHT);
    w.u32(0x0048_0000); // horizresolution
    w.u32(0x0048_0000); // vertresolution
    w.u32(0); // reserved
    w.u16(1); // frame_count
    w.bytes(&[0; 32]); // compressorname
    w.u16(0x0018); // depth
    w.u16(0xffff); // pre_defined = -1
    w.end(); // avc1
    w.end(); // stsd

    // stts
    w.begin(fourcc(b"stts"));
    w.u32(0); // version + flags
    w.u32(1); // entry_count
    w.u32(SIZES.len() as u32); // sample_count
    w.u32(SAMPLE_DELTA); // sample_delta
    w.end();

    // stsc
    w.begin(fourcc(b"stsc"));
    w.u32(0); // version + flags
    w.u32(1); // entry_count
    w.u32(1); // first_chunk
    w.u32(SIZES.len() as u32); // samples_per_chunk
    w.u32(1); // sample_description_index
    w.end();

    // stsz
    w.begin(fourcc(b"stsz"));
    w.u32(0); // version + flags
    w.u32(0); // sample_size = 0 -> per-sample sizes follow
    w.u32(SIZES.len() as u32); // sample_count
    for &sz in &SIZES {
        w.u32(sz);
    }
    w.end();

    // stco
    w.begin(fourcc(b"stco"));
    w.u32(0); // version + flags
    w.u32(1); // entry_count
    w.u32(data_offset); // chunk offset
    w.end();

    // stss (sample 1 is a sync sample)
    w.begin(fourcc(b"stss"));
    w.u32(0); // version + flags
    w.u32(1); // entry_count
    w.u32(1); // sync sample number
    w.end();

    w.end(); // stbl
    w.end(); // minf
    w.end(); // mdia
    w.end(); // trak
    w.end(); // moov
    let moov = w.into_bytes();

    [ftyp, mdat, moov].concat()
}

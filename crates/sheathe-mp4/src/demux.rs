//! MP4 (ISO-BMFF) demuxer: parse `moov` into tracks and extract samples.
//!
//! This is the read side that feeds the packager — Shaka Packager's
//! `media/formats/mp4` parser. It reads the movie header well enough to describe
//! every track ([`StreamInfo`]) and to reconstruct each track's samples from the
//! classic sample tables (`stts`/`ctts`/`stsc`/`stsz`/`stco`/`co64`/`stss`).
//!
//! Codec-specific configuration records (`avcC`, `esds`, …) are summarised into
//! RFC 6381 codec strings by the `codecs` module and carried through verbatim by
//! the writer; the demuxer itself does not expose them as structured fields yet
//! (see `ROADMAP.md`).

use crate::box_reader::{Cursor, Mp4Box, top_level};
use sheathe_core::{Codec, Error, MediaKind, Result, Sample, SampleFlags, StreamInfo, Timescale};

/// A demuxed MP4: the parsed tracks plus a borrow of the source bytes so
/// [`Mp4Demuxer::samples`] can slice out coded sample data.
pub struct Mp4Demuxer<'a> {
    data: &'a [u8],
    tracks: Vec<Track>,
}

/// One track: its describing [`StreamInfo`] plus the raw sample table needed to
/// reconstruct samples.
#[derive(Debug, Clone)]
pub struct Track {
    /// Format-agnostic description of the elementary stream.
    pub info: StreamInfo,
    /// The `tkhd` track identifier.
    pub track_id: u32,
    /// Total number of samples in the track.
    pub sample_count: u32,
    /// The raw `stsd` sample-entry box (e.g. `avc1`/`mp4a`, including its
    /// `avcC`/`esds` codec configuration). Re-emitted verbatim into the CMAF
    /// init segment so the output is playable without rebuilding the config.
    sample_entry: Vec<u8>,
    table: SampleTable,
}

impl Track {
    /// The raw sample-entry box bytes (header included).
    pub fn sample_entry(&self) -> &[u8] {
        &self.sample_entry
    }
}

#[derive(Debug, Clone, Default)]
struct SampleTable {
    /// time-to-sample: (run length, per-sample delta in media timescale).
    stts: Vec<(u32, u32)>,
    /// composition offsets: (run length, signed offset). Empty when absent.
    ctts: Vec<(u32, i64)>,
    /// sample-to-chunk: (first_chunk, samples_per_chunk, sample_description_index).
    stsc: Vec<(u32, u32, u32)>,
    /// per-sample byte sizes.
    sizes: Vec<u32>,
    /// per-chunk byte offsets into the file.
    chunk_offsets: Vec<u64>,
    /// 1-based sync (key) sample numbers; empty means every sample is sync.
    sync: Vec<u32>,
}

impl<'a> Mp4Demuxer<'a> {
    /// Parse the box tree of `data` and extract every track.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let moov = find_top(data, b"moov")?
            .ok_or_else(|| Error::malformed("no moov box (is this fragmented-only or not MP4?)"))?;

        let mut tracks = Vec::new();
        for child in moov.children() {
            let child = child?;
            if &child.kind == b"trak" {
                if let Some(track) = parse_trak(&child)? {
                    tracks.push(track);
                }
            }
        }
        if tracks.is_empty() {
            return Err(Error::malformed("moov contained no readable tracks"));
        }
        Ok(Self { data, tracks })
    }

    /// The parsed tracks.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Reconstruct the samples of track `index`, in decode order, copying each
    /// sample's coded bytes out of the source buffer.
    pub fn samples(&self, index: usize) -> Result<Vec<Sample>> {
        let track =
            self.tracks.get(index).ok_or_else(|| Error::malformed("track index out of range"))?;
        track.table.build_samples(self.data)
    }
}

/// Find a top-level box by type.
fn find_top<'a>(data: &'a [u8], kind: &[u8; 4]) -> Result<Option<Mp4Box<'a>>> {
    for b in top_level(data) {
        let b = b?;
        if &b.kind == kind {
            return Ok(Some(b));
        }
    }
    Ok(None)
}

fn parse_trak(trak: &Mp4Box<'_>) -> Result<Option<Track>> {
    let tkhd = trak.child(b"tkhd")?.ok_or_else(|| Error::malformed("trak without tkhd"))?;
    let track_id = parse_tkhd_track_id(tkhd.body)?;

    let mdia = trak.child(b"mdia")?.ok_or_else(|| Error::malformed("trak without mdia"))?;
    let mdhd = mdia.child(b"mdhd")?.ok_or_else(|| Error::malformed("mdia without mdhd"))?;
    let timescale = parse_mdhd_timescale(mdhd.body)?;

    let hdlr = mdia.child(b"hdlr")?.ok_or_else(|| Error::malformed("mdia without hdlr"))?;
    let handler = parse_hdlr_handler(hdlr.body)?;
    let kind = match &handler {
        b"vide" => MediaKind::Video,
        b"soun" => MediaKind::Audio,
        b"text" | b"sbtl" | b"subt" => MediaKind::Text,
        // Unknown handler (hint, meta, …) — skip rather than fail the whole file.
        _ => return Ok(None),
    };

    let minf = mdia.child(b"minf")?.ok_or_else(|| Error::malformed("mdia without minf"))?;
    let stbl = minf.child(b"stbl")?.ok_or_else(|| Error::malformed("minf without stbl"))?;

    let stsd = stbl.child(b"stsd")?.ok_or_else(|| Error::malformed("stbl without stsd"))?;
    let entry = parse_stsd(stsd.body, kind)?;

    let table = parse_sample_table(&stbl)?;
    let sample_count = table.sizes.len() as u32;
    let bitrate = average_bitrate(&table, timescale);

    let info = StreamInfo {
        kind,
        codec: entry.codec,
        timescale: Timescale(timescale),
        resolution: entry.resolution,
        sample_rate: entry.sample_rate,
        bitrate,
        codec_string: entry.codec_string,
    };

    Ok(Some(Track { info, track_id, sample_count, sample_entry: entry.raw, table }))
}

fn parse_tkhd_track_id(body: &[u8]) -> Result<u32> {
    let mut c = Cursor::new(body);
    let version = c.version_flags()?;
    if version == 1 {
        c.skip(16)?; // creation + modification (8+8)
    } else {
        c.skip(8)?; // creation + modification (4+4)
    }
    c.u32() // track_id
}

fn parse_mdhd_timescale(body: &[u8]) -> Result<u32> {
    let mut c = Cursor::new(body);
    let version = c.version_flags()?;
    if version == 1 {
        c.skip(16)?; // creation + modification (8+8)
    } else {
        c.skip(8)?; // creation + modification (4+4)
    }
    let timescale = c.u32()?;
    if timescale == 0 {
        return Err(Error::malformed("mdhd timescale is zero"));
    }
    Ok(timescale)
}

fn parse_hdlr_handler(body: &[u8]) -> Result<[u8; 4]> {
    let mut c = Cursor::new(body);
    c.version_flags()?;
    c.skip(4)?; // pre_defined
    c.fourcc() // handler_type
}

/// Codec and presentation details parsed from the first `stsd` sample entry.
struct StsdInfo {
    codec: Codec,
    /// RFC 6381 codec string, if the configuration record was parsed.
    codec_string: Option<String>,
    resolution: Option<(u32, u32)>,
    sample_rate: Option<u32>,
    /// The full sample-entry box, reconstructed with a 32-bit size header.
    raw: Vec<u8>,
}

/// Parse `stsd` and return the codec + presentation info of its first entry.
fn parse_stsd(body: &[u8], kind: MediaKind) -> Result<StsdInfo> {
    let mut c = Cursor::new(body);
    c.version_flags()?;
    let entry_count = c.u32()?;
    if entry_count == 0 {
        return Err(Error::malformed("stsd has no sample entries"));
    }
    // After the full-box header (4) + entry_count (4) the body is a sequence of
    // sample-entry boxes; take the first.
    if body.len() < 8 {
        return Err(Error::malformed("stsd truncated"));
    }
    let entry =
        top_level(&body[8..]).next().ok_or_else(|| Error::malformed("stsd entry missing"))??;
    let codec = codec_from_fourcc(&entry.kind);
    let codec_string = crate::codecs::rfc6381(kind, &entry.kind, entry.body);

    // Reconstruct the exact sample-entry box (32-bit size + type + body) so it
    // can be re-emitted verbatim into the init segment.
    let mut raw = Vec::with_capacity(entry.body.len() + 8);
    raw.extend_from_slice(&((entry.body.len() + 8) as u32).to_be_bytes());
    raw.extend_from_slice(&entry.kind);
    raw.extend_from_slice(entry.body);

    let (resolution, sample_rate) = match kind {
        MediaKind::Video => (parse_visual_entry(entry.body)?, None),
        MediaKind::Audio => (None, parse_audio_entry(entry.body)?),
        MediaKind::Text => (None, None),
    };
    Ok(StsdInfo { codec, codec_string, resolution, sample_rate, raw })
}

fn codec_from_fourcc(f: &[u8; 4]) -> Codec {
    match f {
        b"avc1" | b"avc3" => Codec::H264,
        b"hvc1" | b"hev1" => Codec::H265,
        b"av01" => Codec::Av1,
        b"mp4a" => Codec::Aac,
        b"ac-3" => Codec::Ac3,
        b"ec-3" => Codec::Eac3,
        b"Opus" => Codec::Opus,
        b"wvtt" => Codec::WebVtt,
        other => Codec::Other(String::from_utf8_lossy(other).into_owned()),
    }
}

/// VisualSampleEntry: width/height live at body offsets 24/26.
fn parse_visual_entry(body: &[u8]) -> Result<Option<(u32, u32)>> {
    let mut c = Cursor::new(body);
    c.skip(24)?; // 8 SampleEntry + 16 (predefined/reserved/predefined[3])
    let w = c.u16()?;
    let h = c.u16()?;
    Ok(Some((u32::from(w), u32::from(h))))
}

/// AudioSampleEntry: sample rate is a 16.16 fixed-point at body offset 24.
fn parse_audio_entry(body: &[u8]) -> Result<Option<u32>> {
    let mut c = Cursor::new(body);
    c.skip(24)?; // 8 SampleEntry + 8 reserved + channelcount/samplesize/predef/reserved (8)
    let rate_fixed = c.u32()?;
    Ok(Some(rate_fixed >> 16))
}

fn parse_sample_table(stbl: &Mp4Box<'_>) -> Result<SampleTable> {
    let mut t = SampleTable::default();

    if let Some(b) = stbl.child(b"stts")? {
        let mut c = Cursor::new(b.body);
        c.version_flags()?;
        let n = c.u32()?;
        for _ in 0..n {
            let count = c.u32()?;
            let delta = c.u32()?;
            t.stts.push((count, delta));
        }
    }

    if let Some(b) = stbl.child(b"ctts")? {
        let mut c = Cursor::new(b.body);
        let version = c.version_flags()?;
        let n = c.u32()?;
        for _ in 0..n {
            let count = c.u32()?;
            let off = if version == 1 { i64::from(c.u32()? as i32) } else { i64::from(c.u32()?) };
            t.ctts.push((count, off));
        }
    }

    let stsc = stbl.child(b"stsc")?.ok_or_else(|| Error::malformed("stbl without stsc"))?;
    {
        let mut c = Cursor::new(stsc.body);
        c.version_flags()?;
        let n = c.u32()?;
        for _ in 0..n {
            let first_chunk = c.u32()?;
            let spc = c.u32()?;
            let desc = c.u32()?;
            t.stsc.push((first_chunk, spc, desc));
        }
    }

    let stsz = stbl.child(b"stsz")?;
    let stz2 = stbl.child(b"stz2")?;
    match (stsz, stz2) {
        (Some(b), _) => {
            let mut c = Cursor::new(b.body);
            c.version_flags()?;
            let uniform = c.u32()?;
            let count = c.u32()?;
            if uniform != 0 {
                t.sizes = vec![uniform; count as usize];
            } else {
                t.sizes.reserve(count as usize);
                for _ in 0..count {
                    t.sizes.push(c.u32()?);
                }
            }
        }
        (None, Some(b)) => {
            let mut c = Cursor::new(b.body);
            c.version_flags()?;
            let field_size = (c.u32()? & 0xff) as u8; // low byte
            let count = c.u32()?;
            t.sizes = read_stz2_sizes(&mut c, field_size, count)?;
        }
        (None, None) => return Err(Error::malformed("stbl without stsz/stz2")),
    }

    if let Some(b) = stbl.child(b"stco")? {
        let mut c = Cursor::new(b.body);
        c.version_flags()?;
        let n = c.u32()?;
        for _ in 0..n {
            t.chunk_offsets.push(u64::from(c.u32()?));
        }
    } else if let Some(b) = stbl.child(b"co64")? {
        let mut c = Cursor::new(b.body);
        c.version_flags()?;
        let n = c.u32()?;
        for _ in 0..n {
            t.chunk_offsets.push(c.u64()?);
        }
    } else {
        return Err(Error::malformed("stbl without stco/co64"));
    }

    if let Some(b) = stbl.child(b"stss")? {
        let mut c = Cursor::new(b.body);
        c.version_flags()?;
        let n = c.u32()?;
        for _ in 0..n {
            t.sync.push(c.u32()?);
        }
    }

    Ok(t)
}

/// `stz2` packs sizes in 4-, 8-, or 16-bit fields.
fn read_stz2_sizes(c: &mut Cursor<'_>, field_size: u8, count: u32) -> Result<Vec<u32>> {
    let count = count as usize;
    let mut sizes = Vec::with_capacity(count);
    match field_size {
        16 => {
            for _ in 0..count {
                sizes.push(u32::from(c.u16()?));
            }
        }
        8 => {
            for _ in 0..count {
                sizes.push(u32::from(c.u8()?));
            }
        }
        4 => {
            // Two 4-bit sizes per byte, high nibble first.
            for i in 0..count {
                if i % 2 == 0 {
                    let byte = c.u8()?;
                    sizes.push(u32::from(byte >> 4));
                    if i + 1 < count {
                        sizes.push(u32::from(byte & 0x0f));
                    }
                }
            }
        }
        other => return Err(Error::malformed(format!("unsupported stz2 field size {other}"))),
    }
    Ok(sizes)
}

fn average_bitrate(t: &SampleTable, timescale: u32) -> Option<u32> {
    let total_bytes: u64 = t.sizes.iter().map(|&s| u64::from(s)).sum();
    let total_ticks: u64 = t.stts.iter().map(|&(c, d)| u64::from(c) * u64::from(d)).sum();
    if total_ticks == 0 {
        return None;
    }
    let seconds = total_ticks as f64 / f64::from(timescale);
    if seconds <= 0.0 {
        return None;
    }
    Some(((total_bytes as f64 * 8.0) / seconds) as u32)
}

impl SampleTable {
    fn build_samples(&self, file: &[u8]) -> Result<Vec<Sample>> {
        let n = self.sizes.len();
        if n == 0 {
            return Ok(Vec::new());
        }

        // 1) Per-sample file offset, from stsc + chunk offsets + sizes.
        let offsets = self.sample_offsets()?;

        // 2) Per-sample dts + duration, from stts.
        let (dts, durations) = self.sample_timing(n)?;

        // 3) Per-sample composition offset, from ctts (optional).
        let cts = self.composition_offsets(n);

        // 4) Sync (keyframe) lookup, from stss (optional).
        let is_sync = |i: usize| -> bool {
            if self.sync.is_empty() {
                true
            } else {
                self.sync.binary_search(&(i as u32 + 1)).is_ok()
            }
        };

        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let size = self.sizes[i] as usize;
            let start = offsets[i] as usize;
            let end = start
                .checked_add(size)
                .filter(|&e| e <= file.len())
                .ok_or_else(|| Error::malformed("sample data range out of bounds"))?;
            let mut flags = SampleFlags::empty();
            if is_sync(i) {
                flags.insert(SampleFlags::KEYFRAME);
            }
            let pts = (dts[i] as i64 + cts[i]).max(0) as u64;
            samples.push(Sample {
                dts: dts[i],
                pts,
                duration: durations[i],
                flags,
                data: file[start..end].to_vec(),
            });
        }
        Ok(samples)
    }

    fn sample_offsets(&self) -> Result<Vec<u64>> {
        let n = self.sizes.len();
        let num_chunks = self.chunk_offsets.len();
        if self.stsc.is_empty() || num_chunks == 0 {
            return Err(Error::malformed("empty stsc or chunk offset table"));
        }
        let mut offsets = Vec::with_capacity(n);
        let mut sample_idx = 0usize;
        let mut stsc_idx = 0usize;

        for chunk in 0..num_chunks {
            let chunk_no = chunk as u32 + 1;
            // Advance to the stsc entry governing this chunk.
            while stsc_idx + 1 < self.stsc.len() && self.stsc[stsc_idx + 1].0 <= chunk_no {
                stsc_idx += 1;
            }
            let samples_per_chunk = self.stsc[stsc_idx].1;
            let mut off = self.chunk_offsets[chunk];
            for _ in 0..samples_per_chunk {
                if sample_idx >= n {
                    break;
                }
                offsets.push(off);
                off += u64::from(self.sizes[sample_idx]);
                sample_idx += 1;
            }
            if sample_idx >= n {
                break;
            }
        }

        if offsets.len() != n {
            return Err(Error::malformed(format!(
                "sample/chunk mismatch: mapped {} of {} samples",
                offsets.len(),
                n
            )));
        }
        Ok(offsets)
    }

    fn sample_timing(&self, n: usize) -> Result<(Vec<u64>, Vec<u32>)> {
        let mut dts = Vec::with_capacity(n);
        let mut durations = Vec::with_capacity(n);
        let mut t = 0u64;
        for &(count, delta) in &self.stts {
            for _ in 0..count {
                if dts.len() >= n {
                    break;
                }
                dts.push(t);
                durations.push(delta);
                t += u64::from(delta);
            }
        }
        // Tolerate a short stts by padding the tail with zero-duration samples.
        while dts.len() < n {
            dts.push(t);
            durations.push(0);
        }
        Ok((dts, durations))
    }

    fn composition_offsets(&self, n: usize) -> Vec<i64> {
        if self.ctts.is_empty() {
            return vec![0; n];
        }
        let mut cts = Vec::with_capacity(n);
        for &(count, off) in &self.ctts {
            for _ in 0..count {
                if cts.len() >= n {
                    break;
                }
                cts.push(off);
            }
        }
        cts.resize(n, 0);
        cts
    }
}

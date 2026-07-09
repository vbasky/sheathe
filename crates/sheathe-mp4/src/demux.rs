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
use std::collections::HashMap;

/// A demuxed MP4: the parsed tracks plus a borrow of the source bytes so
/// [`Mp4Demuxer::samples`] can slice out coded sample data.
pub struct Mp4Demuxer<'a> {
    data: &'a [u8],
    tracks: Vec<Track>,
    /// Samples reconstructed from `moof`/`traf`/`trun` fragments, keyed by
    /// `track_id`. Empty for progressive (sample-table) files.
    fragments: HashMap<u32, Vec<Sample>>,
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

    /// Build a track from a pre-built sample entry and an already-extracted
    /// sample list (e.g. MPEG-TS demux). The internal sample table is empty;
    /// callers must supply samples separately.
    pub fn from_sample_entry(
        info: StreamInfo,
        track_id: u32,
        sample_entry: Vec<u8>,
        samples: &[Sample],
    ) -> Self {
        Self {
            info,
            track_id,
            sample_count: samples.len() as u32,
            sample_entry,
            table: SampleTable::default(),
        }
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

        // Fragmented MP4 (CMAF): samples live in `moof`/`trun`, not `stbl`.
        let fragments = parse_fragments(data, &moov)?;
        for t in &mut tracks {
            if let Some(s) = fragments.get(&t.track_id) {
                t.sample_count = s.len() as u32;
            }
        }

        Ok(Self { data, tracks, fragments })
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
        if let Some(frag) = self.fragments.get(&track.track_id) {
            if !frag.is_empty() {
                return Ok(frag.clone());
            }
        }
        track.table.build_samples(self.data)
    }
}

// ---- Fragmented MP4 (moof / traf / trun) ------------------------------------

/// Per-track fallback sample parameters declared once in `mvex`/`trex`.
#[derive(Clone, Copy, Default)]
struct TrexDefaults {
    duration: u32,
    size: u32,
    flags: u32,
}

/// Read a full-box version byte and its 24-bit flags.
fn version_and_flags(c: &mut Cursor<'_>) -> Result<(u8, u32)> {
    let version = c.u8()?;
    let flags = (u32::from(c.u8()?) << 16) | (u32::from(c.u8()?) << 8) | u32::from(c.u8()?);
    Ok((version, flags))
}

/// Byte offset of `sub` within `data` (both must alias the same allocation).
fn offset_in(data: &[u8], sub: &[u8]) -> usize {
    (sub.as_ptr() as usize).saturating_sub(data.as_ptr() as usize)
}

/// Reconstruct samples from every `moof` in the file, keyed by `track_id`.
/// Returns an empty map for progressive (non-fragmented) files.
fn parse_fragments(data: &[u8], moov: &Mp4Box<'_>) -> Result<HashMap<u32, Vec<Sample>>> {
    let mut trex: HashMap<u32, TrexDefaults> = HashMap::new();
    if let Some(mvex) = moov.child(b"mvex")? {
        for b in mvex.children() {
            let b = b?;
            if &b.kind != b"trex" {
                continue;
            }
            let mut c = Cursor::new(b.body);
            version_and_flags(&mut c)?;
            let track_id = c.u32()?;
            c.u32()?; // default_sample_description_index
            let duration = c.u32()?;
            let size = c.u32()?;
            let flags = c.u32()?;
            trex.insert(track_id, TrexDefaults { duration, size, flags });
        }
    }

    let mut out: HashMap<u32, Vec<Sample>> = HashMap::new();
    let mut decode_time: HashMap<u32, u64> = HashMap::new();
    for b in top_level(data) {
        let b = b?;
        if &b.kind != b"moof" {
            continue;
        }
        let moof_start = offset_in(data, b.body).saturating_sub(8);
        for traf in b.children() {
            let traf = traf?;
            if &traf.kind == b"traf" {
                parse_traf(data, &traf, moof_start, &trex, &mut decode_time, &mut out)?;
            }
        }
    }
    Ok(out)
}

fn parse_traf(
    data: &[u8],
    traf: &Mp4Box<'_>,
    moof_start: usize,
    trex: &HashMap<u32, TrexDefaults>,
    decode_time: &mut HashMap<u32, u64>,
    out: &mut HashMap<u32, Vec<Sample>>,
) -> Result<()> {
    let tfhd = traf.child(b"tfhd")?.ok_or_else(|| Error::malformed("traf without tfhd"))?;
    let mut c = Cursor::new(tfhd.body);
    let (_v, flags) = version_and_flags(&mut c)?;
    let track_id = c.u32()?;
    // default-base-is-moof and the common no-base case both anchor at the moof.
    let mut base = moof_start as u64;
    if flags & 0x00_0001 != 0 {
        base = c.u64()?; // base-data-offset-present
    }
    if flags & 0x00_0002 != 0 {
        c.u32()?; // sample-description-index-present
    }
    let def = trex.get(&track_id).copied().unwrap_or_default();
    let default_duration = if flags & 0x00_0008 != 0 { c.u32()? } else { def.duration };
    let default_size = if flags & 0x00_0010 != 0 { c.u32()? } else { def.size };
    let default_flags = if flags & 0x00_0020 != 0 { c.u32()? } else { def.flags };

    let mut dts = *decode_time.get(&track_id).unwrap_or(&0);
    if let Some(tfdt) = traf.child(b"tfdt")? {
        let mut t = Cursor::new(tfdt.body);
        let (v, _f) = version_and_flags(&mut t)?;
        dts = if v == 1 { t.u64()? } else { u64::from(t.u32()?) };
    }

    let samples = out.entry(track_id).or_default();
    for trun in traf.children() {
        let trun = trun?;
        if &trun.kind != b"trun" {
            continue;
        }
        let mut c = Cursor::new(trun.body);
        let (_v, tflags) = version_and_flags(&mut c)?;
        let count = c.u32()?;
        let mut off = base;
        if tflags & 0x00_0001 != 0 {
            let data_offset = c.u32()? as i32; // data-offset-present (signed, from `base`)
            off = (base as i64 + i64::from(data_offset)) as u64;
        }
        let first_flags = if tflags & 0x00_0004 != 0 { Some(c.u32()?) } else { None };

        for i in 0..count {
            let duration = if tflags & 0x00_0100 != 0 { c.u32()? } else { default_duration };
            let size = if tflags & 0x00_0200 != 0 { c.u32()? } else { default_size };
            let sflags = if tflags & 0x00_0400 != 0 {
                c.u32()?
            } else if i == 0 {
                first_flags.unwrap_or(default_flags)
            } else {
                default_flags
            };
            let cto: i64 = if tflags & 0x00_0800 != 0 { i64::from(c.u32()? as i32) } else { 0 };

            let start = off as usize;
            let end = start
                .checked_add(size as usize)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| Error::malformed("fragment sample out of bounds"))?;
            let mut sfl = SampleFlags::empty();
            if sflags & 0x0001_0000 == 0 {
                sfl.insert(SampleFlags::KEYFRAME); // sample_is_non_sync_sample clear
            }
            let pts = (dts as i64 + cto).max(0) as u64;
            samples.push(Sample {
                dts,
                pts,
                duration,
                flags: sfl,
                data: data[start..end].to_vec(),
            });
            dts = dts.saturating_add(u64::from(duration));
            off = end as u64;
        }
    }
    decode_time.insert(track_id, dts);
    Ok(())
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
        b"vp08" => Codec::Vp8,
        b"vp09" => Codec::Vp9,
        b"mp4a" => Codec::Aac,
        b"ac-3" => Codec::Ac3,
        b"ec-3" => Codec::Eac3,
        b".mp3" | b"mp3 " => Codec::Mp3,
        b"fLaC" => Codec::Flac,
        b"Opus" => Codec::Opus,
        b"wvtt" => Codec::WebVtt,
        b"stpp" => Codec::Stpp,
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

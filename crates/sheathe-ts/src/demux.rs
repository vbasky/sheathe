//! MPEG-TS demuxer: PAT/PMT/PES → tracks and samples.

use crate::elementary::{self, ElementaryTrack};
use crate::packet::packets;
use crate::pes::parse_pes;
use crate::psi::{Pat, Pmt, parse_pat, parse_pmt, strip_pointer};
use sheathe_core::{Error, Result};
use std::collections::HashMap;

/// Stream type constants (ISO/IEC 13818-1 Table 2-34).
const STREAM_TYPE_AVC: u8 = 0x1b;
const STREAM_TYPE_AAC_ADTS: u8 = 0x0f;
const STREAM_TYPE_HEVC: u8 = 0x24;

#[derive(Debug, Default)]
struct PesBuffer {
    data: Vec<u8>,
    pts: Option<u64>,
    dts: Option<u64>,
}

/// A demuxed transport stream.
pub struct TsDemuxer {
    tracks: Vec<TsTrack>,
}

/// One elementary stream extracted from a transport stream.
#[derive(Debug, Clone)]
pub struct TsTrack {
    /// Format-agnostic stream description.
    pub info: sheathe_core::StreamInfo,
    /// Elementary-stream PID.
    pub pid: u16,
    /// Demuxed samples in decode order.
    pub samples: Vec<sheathe_core::Sample>,
    /// `avc1`/`mp4a` sample-entry box bytes for CMAF init segments.
    pub sample_entry: Vec<u8>,
}

impl From<ElementaryTrack> for TsTrack {
    fn from(t: ElementaryTrack) -> Self {
        Self {
            info: t.info,
            pid: 0,
            samples: t.samples,
            sample_entry: t.sample_entry,
        }
    }
}

impl TsDemuxer {
    /// Parse `data` as a transport stream and extract elementary tracks.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut pat: Option<Pat> = None;
        let mut pmts: Vec<Pmt> = Vec::new();
        let mut stream_type_for: HashMap<u16, u8> = HashMap::new();
        let mut in_progress: HashMap<u16, PesBuffer> = HashMap::new();
        let mut completed: HashMap<u16, Vec<PesBuffer>> = HashMap::new();

        for pkt in packets(data) {
            let pkt = pkt?;
            if pkt.pid == 0x0000 && pkt.payload_unit_start {
                let body = strip_pointer(pkt.payload)?.0;
                pat = Some(parse_pat(body)?);
                continue;
            }
            if let Some(ref pat) = pat {
                let is_pmt = pat.programs.iter().any(|&(_, pmt_pid)| pmt_pid == pkt.pid);
                if is_pmt && pkt.payload_unit_start {
                    let body = strip_pointer(pkt.payload)?.0;
                    let pmt = parse_pmt(body)?;
                    for &(st, pid) in &pmt.streams {
                        stream_type_for.insert(pid, st);
                    }
                    pmts.push(pmt);
                    continue;
                }
            }
            if !stream_type_for.contains_key(&pkt.pid) {
                continue;
            }

            if pkt.payload_unit_start {
                if let Some(prev) = in_progress.remove(&pkt.pid) {
                    if !prev.data.is_empty() {
                        completed.entry(pkt.pid).or_default().push(prev);
                    }
                }
                let (body, _) = strip_pointer(pkt.payload)?;
                let mut buf = PesBuffer::default();
                if body.len() >= 6 && body[0..3] == [0x00, 0x00, 0x01] {
                    let stream_id = body[3];
                    if let Ok(hdr) = parse_pes(body, stream_id) {
                        buf.pts = hdr.pts;
                        buf.dts = hdr.dts;
                        buf.data.extend_from_slice(&body[hdr.data_offset..]);
                    }
                }
                in_progress.insert(pkt.pid, buf);
            } else if let Some(buf) = in_progress.get_mut(&pkt.pid) {
                buf.data.extend_from_slice(pkt.payload);
            }
        }

        for (pid, buf) in in_progress {
            if !buf.data.is_empty() {
                completed.entry(pid).or_default().push(buf);
            }
        }

        if pat.is_none() {
            return Err(Error::malformed("no PAT found in transport stream"));
        }
        if pmts.is_empty() {
            return Err(Error::malformed("no PMT found in transport stream"));
        }

        let mut tracks = Vec::new();
        for (pid, stream_type) in &stream_type_for {
            let Some(pes_list) = completed.get(pid) else { continue };
            let payloads: Vec<_> = pes_list.iter().map(|p| (p.data.as_slice(), p.pts, p.dts)).collect();
            let concat: Vec<u8> = pes_list.iter().flat_map(|p| p.data.iter().copied()).collect();
            if let Some(mut track) = build_track(*stream_type, *pid, &concat, &payloads)? {
                track.pid = *pid;
                tracks.push(track);
            }
        }

        if tracks.is_empty() {
            return Err(Error::malformed("transport stream contained no elementary samples"));
        }
        tracks.sort_by_key(|t| t.pid);
        Ok(Self { tracks })
    }

    /// Parsed elementary tracks.
    pub fn tracks(&self) -> &[TsTrack] {
        &self.tracks
    }
}

fn build_track(
    stream_type: u8,
    pid: u16,
    all_data: &[u8],
    pes_payloads: &[(&[u8], Option<u64>, Option<u64>)],
) -> Result<Option<TsTrack>> {
    let elementary = match stream_type {
        STREAM_TYPE_AVC => elementary::h264_from_annex_b(all_data, pes_payloads)?,
        STREAM_TYPE_AAC_ADTS => elementary::aac_adts(all_data)?,
        STREAM_TYPE_HEVC => elementary::hevc_from_annex_b(all_data, pes_payloads)?,
        other => {
            return Err(Error::unsupported(format!(
                "MPEG-TS stream type 0x{other:02x} (PID {pid}) is not supported yet"
            )));
        }
    };
    Ok(Some(elementary.into()))
}
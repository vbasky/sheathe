//! MPEG-2 transport stream **muxer** (188-byte packets).
//!
//! Builds a single-program TS from elementary video (Annex B H.264/H.265) and/or
//! audio (ADTS-AAC / AC-3 syncframes) samples. Used for HLS TS segment output
//! and the Phase 5 packed-container path.

use sheathe_core::{Codec, MediaKind, Sample, StreamInfo};
use std::io::Write;

use crate::packet::{PACKET_SIZE, SYNC_BYTE};

/// Default video PID.
pub const PID_VIDEO: u16 = 0x0100;
/// Default audio PID.
pub const PID_AUDIO: u16 = 0x0101;
/// PMT PID.
pub const PID_PMT: u16 = 0x1000;
/// PCR PID (tied to the first stream).
pub const PID_PCR: u16 = PID_VIDEO;

/// One elementary stream to mux.
#[derive(Debug, Clone)]
pub struct MuxTrack {
    /// Stream metadata (codec, timescale).
    pub info: StreamInfo,
    /// Transport PID for this track.
    pub pid: u16,
    /// Samples in decode order.
    pub samples: Vec<Sample>,
}

/// Build a single-program MPEG-TS byte stream from `tracks`.
///
/// Emits PAT + PMT once, then PES packets for every sample. PCR rides on the
/// first video track (or first track if audio-only). Continuity counters are
/// tracked per PID.
pub fn mux_program(tracks: &[MuxTrack]) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(tracks.iter().map(|t| t.samples.len() * 4 * PACKET_SIZE).sum());
    let mut cc = Continuity::default();

    // PAT (PID 0) + PMT.
    write_psi_packet(&mut out, 0x0000, &build_pat(), &mut cc);
    write_psi_packet(&mut out, PID_PMT, &build_pmt(tracks), &mut cc);

    // Interleave by DTS converted to a common 90 kHz timeline.
    let mut events: Vec<(u64, usize, usize)> = Vec::new(); // (dts_90k, track, sample)
    for (ti, t) in tracks.iter().enumerate() {
        let scale = t.info.timescale.0.max(1) as u64;
        for (si, s) in t.samples.iter().enumerate() {
            let dts_90k = s.dts.saturating_mul(90_000) / scale;
            events.push((dts_90k, ti, si));
        }
    }
    events.sort_by_key(|(d, ti, si)| (*d, *ti, *si));

    for (dts_90k, ti, si) in events {
        let track = &tracks[ti];
        let sample = &track.samples[si];
        let pts_90k = {
            let scale = track.info.timescale.0.max(1) as u64;
            sample.pts.saturating_mul(90_000) / scale
        };
        let stream_id = match track.info.kind {
            MediaKind::Video => 0xE0,
            MediaKind::Audio => 0xC0,
            MediaKind::Text => 0xBD,
        };
        let pes = build_pes(stream_id, pts_90k, Some(dts_90k), &sample.data);
        let with_pcr = track.pid == pcr_pid(tracks) && sample.is_segment_boundary();
        write_pes_packets(&mut out, track.pid, &pes, with_pcr.then_some(dts_90k), &mut cc);
    }
    out
}

/// Mux a single track's samples into one TS segment (PAT/PMT + PES).
pub fn mux_segment(track: &MuxTrack) -> Vec<u8> {
    mux_program(std::slice::from_ref(track))
}

fn pcr_pid(tracks: &[MuxTrack]) -> u16 {
    tracks
        .iter()
        .find(|t| t.info.kind == MediaKind::Video)
        .or_else(|| tracks.first())
        .map(|t| t.pid)
        .unwrap_or(PID_PCR)
}

struct Continuity {
    counters: [u8; 8192],
}

impl Default for Continuity {
    fn default() -> Self {
        Self { counters: [0; 8192] }
    }
}

impl Continuity {
    fn next(&mut self, pid: u16) -> u8 {
        let i = pid as usize & 0x1fff;
        let c = self.counters[i] & 0x0f;
        self.counters[i] = (c + 1) & 0x0f;
        c
    }
}

fn write_psi_packet(out: &mut Vec<u8>, pid: u16, section: &[u8], cc: &mut Continuity) {
    let mut pkt = [0xffu8; PACKET_SIZE];
    pkt[0] = SYNC_BYTE;
    // payload_unit_start=1, pid
    pkt[1] = 0x40 | ((pid >> 8) as u8 & 0x1f);
    pkt[2] = (pid & 0xff) as u8;
    pkt[3] = 0x10 | cc.next(pid); // payload only
    pkt[4] = 0x00; // pointer_field
    let copy = section.len().min(PACKET_SIZE - 5);
    pkt[5..5 + copy].copy_from_slice(&section[..copy]);
    out.extend_from_slice(&pkt);
}

fn write_pes_packets(
    out: &mut Vec<u8>,
    pid: u16,
    pes: &[u8],
    pcr_90k: Option<u64>,
    cc: &mut Continuity,
) {
    let mut offset = 0;
    let mut first = true;
    while offset < pes.len() {
        let mut pkt = [0xffu8; PACKET_SIZE];
        pkt[0] = SYNC_BYTE;
        let mut hdr = 4usize;
        let pusi = if first { 0x40 } else { 0 };
        pkt[1] = pusi | ((pid >> 8) as u8 & 0x1f);
        pkt[2] = (pid & 0xff) as u8;

        let mut afc = 0x01; // payload only by default
        if first {
            if let Some(pcr) = pcr_90k {
                // adaptation + payload, with PCR
                afc = 0x03;
                let af_len = 7u8; // flags + 6-byte PCR
                pkt[4] = af_len;
                pkt[5] = 0x10; // PCR flag
                write_pcr(&mut pkt[6..12], pcr);
                hdr = 4 + 1 + af_len as usize;
            }
        }
        pkt[3] = (afc << 4) | cc.next(pid);

        let space = PACKET_SIZE - hdr;
        let take = (pes.len() - offset).min(space);
        pkt[hdr..hdr + take].copy_from_slice(&pes[offset..offset + take]);
        // Stuff remaining payload space with 0xff already in pkt.
        // If we need stuffing via adaptation (when first has no PCR and short),
        // pad is fine as 0xff payload for PSI-less packets — for PES the remainder
        // is adaptation stuffing:
        if take < space && !(first && pcr_90k.is_some()) {
            // Convert to adaptation+payload with stuffing.
            let stuff = space - take;
            let mut stuffed = [0xffu8; PACKET_SIZE];
            stuffed[0] = SYNC_BYTE;
            stuffed[1] = pusi | ((pid >> 8) as u8 & 0x1f);
            stuffed[2] = (pid & 0xff) as u8;
            // Re-issue continuity: we already consumed one; use previous value
            // by rewinding… simpler: write adaptation with stuffing, same cc
            // already advanced — keep packet as-is with 0xff tail (illegal PES
            // but players tolerate for last packet). Prefer proper stuffing:
            stuffed[3] = 0x30 | (pkt[3] & 0x0f); // adapt+payload, same cc
            stuffed[4] = (stuff - 1) as u8; // af length
            if stuff > 1 {
                stuffed[5] = 0x00; // flags
                // rest already 0xff stuffing
            }
            let payload_start = 4 + stuff;
            stuffed[payload_start..payload_start + take]
                .copy_from_slice(&pes[offset..offset + take]);
            out.extend_from_slice(&stuffed);
        } else {
            out.extend_from_slice(&pkt);
        }
        offset += take;
        first = false;
    }
}

fn write_pcr(dst: &mut [u8], pcr_90k: u64) {
    // PCR base is 33 bits of 90 kHz; extension 0.
    let base = pcr_90k & 0x1_ffff_ffff;
    dst[0] = ((base >> 25) & 0xff) as u8;
    dst[1] = ((base >> 17) & 0xff) as u8;
    dst[2] = ((base >> 9) & 0xff) as u8;
    dst[3] = ((base >> 1) & 0xff) as u8;
    dst[4] = (((base & 1) << 7) | 0x7e) as u8; // 6 reserved bits = 1
    dst[5] = 0x00; // extension
}

fn build_pes(stream_id: u8, pts_90k: u64, dts_90k: Option<u64>, payload: &[u8]) -> Vec<u8> {
    let has_dts = dts_90k.is_some() && dts_90k != Some(pts_90k);
    let header_data_len = if has_dts { 10u8 } else { 5u8 };
    let mut pes = Vec::with_capacity(9 + header_data_len as usize + payload.len());
    pes.extend_from_slice(&[0x00, 0x00, 0x01, stream_id]);
    let packet_len = (3 + header_data_len as usize + payload.len()) as u16;
    // 0 means unbounded for video; still write length when it fits.
    if packet_len < 0xffff && stream_id != 0xE0 {
        pes.extend_from_slice(&packet_len.to_be_bytes());
    } else {
        pes.extend_from_slice(&0u16.to_be_bytes());
    }
    // Optional PES header: '10', scrambling=0, data_alignment maybe, PTS(/DTS) flags
    let pts_dts_flags = if has_dts { 0xC0 } else { 0x80 };
    pes.push(0x80); // marker + flags
    pes.push(pts_dts_flags);
    pes.push(header_data_len);
    write_ts_field(&mut pes, if has_dts { 0x3 } else { 0x2 }, pts_90k);
    if has_dts {
        write_ts_field(&mut pes, 0x1, dts_90k.unwrap_or(pts_90k));
    }
    pes.extend_from_slice(payload);
    pes
}

fn write_ts_field(out: &mut Vec<u8>, prefix_nibble: u8, ts: u64) {
    let ts = ts & 0x1_ffff_ffff;
    out.push(((prefix_nibble & 0x0f) << 4) | (((ts >> 29) as u8) << 1) | 1);
    out.push((ts >> 22) as u8);
    out.push(((((ts >> 15) as u8) & 0x7f) << 1) | 1);
    out.push((ts >> 7) as u8);
    out.push((((ts as u8) & 0x7f) << 1) | 1);
}

fn build_pat() -> Vec<u8> {
    // table_id=0, section for program 1 → PMT at PID_PMT
    let mut sec = vec![
        0x00, // table_id
        0xB0,
        0x0D, // syntax=1, length=13
        0x00,
        0x01, // transport_stream_id
        0xC1, // version 0 + current
        0x00,
        0x00, // section/last section
        0x00,
        0x01, // program_number 1
        // program_map_PID
        ((0xE0) | ((PID_PMT >> 8) as u8 & 0x1f)),
        (PID_PMT & 0xff) as u8,
    ];
    let crc = mpeg_crc32(&sec);
    sec.write_all(&crc.to_be_bytes()).ok();
    sec
}

fn build_pmt(tracks: &[MuxTrack]) -> Vec<u8> {
    let pcr = pcr_pid(tracks);
    let mut body = Vec::new();
    // PCR_PID
    body.push(0xE0 | ((pcr >> 8) as u8 & 0x1f));
    body.push((pcr & 0xff) as u8);
    body.extend_from_slice(&0xF000u16.to_be_bytes()); // program_info_length = 0

    for t in tracks {
        let stream_type = match (&t.info.kind, &t.info.codec) {
            (MediaKind::Video, Codec::H264) => 0x1B,
            (MediaKind::Video, Codec::H265) => 0x24,
            (MediaKind::Audio, Codec::Aac) => 0x0F,
            (MediaKind::Audio, Codec::Ac3) => 0x81,
            (MediaKind::Audio, Codec::Eac3) => 0x87,
            (MediaKind::Audio, Codec::Mp3) => 0x03,
            _ => 0x06, // private
        };
        body.push(stream_type);
        body.push(0xE0 | ((t.pid >> 8) as u8 & 0x1f));
        body.push((t.pid & 0xff) as u8);
        body.extend_from_slice(&0xF000u16.to_be_bytes()); // es_info_length = 0
    }

    let mut sec = vec![0x02]; // table_id PMT
    let section_length = (5 + body.len() + 4) as u16; // prog_num… + body + CRC
    let sl = 0xB000 | (section_length & 0x0fff);
    sec.extend_from_slice(&sl.to_be_bytes());
    sec.extend_from_slice(&0x0001u16.to_be_bytes()); // program_number
    sec.push(0xC1); // version + current
    sec.push(0x00);
    sec.push(0x00);
    sec.extend_from_slice(&body);
    let crc = mpeg_crc32(&sec);
    sec.extend_from_slice(&crc.to_be_bytes());
    sec
}

fn mpeg_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            if crc & 0x8000_0000 != 0 {
                crc = (crc << 1) ^ 0x04C1_1DB7;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheathe_core::{SampleFlags, Timescale};

    fn dummy_video() -> MuxTrack {
        MuxTrack {
            info: StreamInfo {
                kind: MediaKind::Video,
                codec: Codec::H264,
                timescale: Timescale(90000),
                resolution: Some((320, 240)),
                sample_rate: None,
                bitrate: Some(500_000),
                codec_string: Some("avc1.42E01E".into()),
            },
            pid: PID_VIDEO,
            samples: vec![Sample {
                dts: 0,
                pts: 0,
                duration: 3000,
                flags: SampleFlags::KEYFRAME,
                data: vec![0x00, 0x00, 0x00, 0x01, 0x09, 0x10], // AUD-ish
            }],
        }
    }

    #[test]
    fn mux_emits_sync_packets_and_pat() {
        let ts = mux_segment(&dummy_video());
        assert!(ts.len() >= PACKET_SIZE * 3);
        assert!(ts.len().is_multiple_of(PACKET_SIZE));
        assert_eq!(ts[0], SYNC_BYTE);
        // Second packet should also sync.
        assert_eq!(ts[PACKET_SIZE], SYNC_BYTE);
        // PAT payload pointer after header.
        assert_eq!(ts[4], 0x00);
    }

    #[test]
    fn mux_program_two_tracks() {
        let mut a = dummy_video();
        let audio = MuxTrack {
            info: StreamInfo {
                kind: MediaKind::Audio,
                codec: Codec::Aac,
                timescale: Timescale(48000),
                resolution: None,
                sample_rate: Some(48000),
                bitrate: Some(128_000),
                codec_string: Some("mp4a.40.2".into()),
            },
            pid: PID_AUDIO,
            samples: vec![Sample {
                dts: 0,
                pts: 0,
                duration: 1024,
                flags: SampleFlags::empty(),
                data: vec![0xFF, 0xF1, 0x50, 0x80, 0x01, 0x3F, 0xFC, 0x00],
            }],
        };
        a.samples.push(Sample {
            dts: 3000,
            pts: 3000,
            duration: 3000,
            flags: SampleFlags::KEYFRAME,
            data: vec![0x00, 0x00, 0x01, 0x65],
        });
        let ts = mux_program(&[a, audio]);
        assert!(ts.len() > PACKET_SIZE * 4);
        assert!(ts.chunks_exact(PACKET_SIZE).all(|p| p[0] == SYNC_BYTE));
    }
}

//! Synthetic MPEG-TS fixture builder for hermetic demux tests.

#![allow(unreachable_pub)]

use sheathe_ts::packet::PACKET_SIZE;

pub const PMT_PID: u16 = 0x0100;
pub const VIDEO_PID: u16 = 0x0101;
pub const AUDIO_PID: u16 = 0x0102;

/// Minimal H.264 SPS/PPS/IDR NAL units (Annex B).
pub fn h264_access_unit() -> Vec<u8> {
    let sps = [0x67, 0x42, 0x00, 0x1e, 0x96, 0x54, 0x05, 0x01, 0xed, 0x80];
    let pps = [0x68, 0xce, 0x3c, 0x80];
    let idr = [0x65, 0x88, 0x84, 0x00, 0x10];
    let mut out = Vec::new();
    for nal in [&sps[..], &pps[..], &idr[..]] {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

/// Minimal ADTS AAC frame (44.1 kHz, stereo, 7-byte frame).
pub fn adts_frame() -> Vec<u8> {
    // profile=1 (AAC-LC), sr_idx=4 (44100), channels=2, aac_frame_length=7
    vec![0xff, 0xf1, 0x50, 0x80, 0x00, 0xe0, 0x00]
}

/// Build a transport stream with PAT, PMT, and one H.264 video PES.
pub fn build_ts_video() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&ts_packet(0x0000, true, &pat_section()));
    out.extend_from_slice(&ts_packet(PMT_PID, true, &pmt_section(&[(0x1b, VIDEO_PID)])));
    let pes = pes_packet(0xe0, &h264_access_unit(), Some(90_000), Some(90_000));
    for (i, chunk) in pes.chunks(184).enumerate() {
        let mut payload = vec![0];
        if i == 0 {
            payload[0] = 0;
        }
        payload.extend_from_slice(chunk);
        payload.resize(184, 0xff);
        out.extend_from_slice(&ts_packet(VIDEO_PID, i == 0, &payload));
    }
    out
}

/// Minimal HEVC VPS/SPS/PPS/IDR NAL units (Annex B, Main profile).
pub fn hevc_access_unit() -> Vec<u8> {
    let vps = [
        0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00,
        0x03, 0x00, 0x00, 0x03, 0x00, 0x78, 0x95, 0x98, 0x09,
    ];
    let sps = [
        0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x78, 0xa0, 0x03, 0xc0, 0x80, 0x10, 0xe5, 0x96, 0x66, 0x69, 0x24, 0xca, 0xe0,
        0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x01, 0xe0, 0x80,
    ];
    let pps = [0x44, 0x01, 0xc1, 0x72, 0xb4, 0x62, 0x40];
    let idr = [0x26, 0x01, 0xaf, 0x09, 0x40, 0x00];
    let mut out = Vec::new();
    for nal in [&vps[..], &sps[..], &pps[..], &idr[..]] {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

/// Build a transport stream with PAT, PMT, and one HEVC video PES.
pub fn build_ts_hevc() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&ts_packet(0x0000, true, &pat_section()));
    out.extend_from_slice(&ts_packet(PMT_PID, true, &pmt_section(&[(0x24, VIDEO_PID)])));
    let pes = pes_packet(0xe0, &hevc_access_unit(), Some(90_000), Some(90_000));
    for (i, chunk) in pes.chunks(184).enumerate() {
        let mut payload = vec![0];
        if i == 0 {
            payload[0] = 0;
        }
        payload.extend_from_slice(chunk);
        payload.resize(184, 0xff);
        out.extend_from_slice(&ts_packet(VIDEO_PID, i == 0, &payload));
    }
    out
}

/// Build a transport stream with PAT, PMT, and one ADTS-AAC audio PES.
pub fn build_ts_aac() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&ts_packet(0x0000, true, &pat_section()));
    out.extend_from_slice(&ts_packet(
        PMT_PID,
        true,
        &pmt_section(&[(0x0f, AUDIO_PID)]),
    ));
    let pes = pes_packet(0xc0, &adts_frame(), Some(90_000), Some(90_000));
    for (i, chunk) in pes.chunks(184).enumerate() {
        let mut payload = vec![0];
        if i == 0 {
            payload[0] = 0;
        }
        payload.extend_from_slice(chunk);
        payload.resize(184, 0xff);
        out.extend_from_slice(&ts_packet(AUDIO_PID, i == 0, &payload));
    }
    out
}

fn pes_packet(stream_id: u8, data: &[u8], pts: Option<u64>, dts: Option<u64>) -> Vec<u8> {
    let mut hdr = vec![0x00, 0x00, 0x01, stream_id, 0x00, 0x00];
    hdr.push(0x80); // marker bits
    let pts_flag = pts.is_some();
    let dts_flag = dts.is_some() && dts != pts;
    hdr.push(if dts_flag { 0xc0 } else if pts_flag { 0x80 } else { 0x00 });
    let mut extra = Vec::new();
    if let Some(p) = pts {
        extra.extend_from_slice(&write_timestamp(0x20, p));
    }
    if let Some(d) = dts {
        extra.extend_from_slice(&write_timestamp(0x10, d));
    }
    hdr.push(extra.len() as u8);
    hdr.extend_from_slice(&extra);
    hdr.extend_from_slice(data);
    hdr
}

fn write_timestamp(marker: u8, ts: u64) -> [u8; 5] {
    [
        marker | (((ts >> 30) & 0x07) as u8) << 1 | 1,
        ((ts >> 22) & 0xff) as u8,
        (((ts >> 15) & 0x7f) as u8) << 1 | 1,
        ((ts >> 7) & 0xff) as u8,
        (((ts & 0x7f) as u8) << 1) | 1,
    ]
}

fn pat_section() -> Vec<u8> {
    let mut body = vec![
        0x00, // table_id
        0xb0, 0x0d, // section_syntax + length placeholder
        0x00, 0x01, // transport_stream_id
        0xc1, // version/current
        0x00, 0x00, // section numbers
        0x00, 0x01, // program 1
        (PMT_PID >> 8) as u8 | 0xe0,
        (PMT_PID & 0xff) as u8,
    ];
    let len = body.len() - 3 + 4;
    body[2] = (len & 0xff) as u8;
    body[1] = 0xb0 | ((len >> 8) as u8 & 0x0f);
    append_crc(&mut body);
    body
}

fn pmt_section(streams: &[(u8, u16)]) -> Vec<u8> {
    let mut body = vec![
        0x02, // table_id
        0xb0, 0x00, // length patched below
        0x00, 0x01, // program_number
        0xc1, // version/current
        0x00, 0x00, // section numbers
        (PMT_PID >> 8) as u8 | 0xe0,
        (PMT_PID & 0xff) as u8,
        0xf0, 0x00, // PCR PID + program_info_length
    ];
    for &(st, pid) in streams {
        body.push(st);
        body.push((pid >> 8) as u8 | 0xe0);
        body.push((pid & 0xff) as u8);
        body.extend_from_slice(&[0xf0, 0x00]); // ES_info_length
    }
    let len = body.len() - 3 + 4;
    body[2] = (len & 0xff) as u8;
    body[1] = 0xb0 | ((len >> 8) as u8 & 0x0f);
    append_crc(&mut body);
    body
}

fn append_crc(section: &mut Vec<u8>) {
    let crc = mpeg_crc32(section);
    section.extend_from_slice(&crc.to_be_bytes());
}

fn mpeg_crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C11DB7
            } else {
                crc << 1
            };
        }
    }
    !crc
}

fn ts_packet(pid: u16, payload_start: bool, payload: &[u8]) -> [u8; PACKET_SIZE] {
    let mut pkt = [0xffu8; PACKET_SIZE];
    pkt[0] = 0x47;
    pkt[1] = ((pid >> 8) as u8 & 0x1f) | if payload_start { 0x40 } else { 0x00 };
    pkt[2] = (pid & 0xff) as u8;
    pkt[3] = 0x10; // payload only
    let n = payload.len().min(184);
    pkt[4..4 + n].copy_from_slice(&payload[..n]);
    pkt
}
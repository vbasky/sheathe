//! Packetized elementary stream (PES) header parsing.

use sheathe_core::{Error, Result};

/// Parsed PES header timestamps and payload offset.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PesHeader {
    /// Presentation timestamp in 90 kHz units, if present.
    pub pts: Option<u64>,
    /// Decode timestamp in 90 kHz units, if present.
    pub dts: Option<u64>,
    /// Byte offset into `payload` where elementary stream data begins.
    pub data_offset: usize,
}

/// Parse a PES packet header from `payload` (after any pointer field).
pub(crate) fn parse_pes(payload: &[u8], stream_id: u8) -> Result<PesHeader> {
    if payload.len() < 9 {
        return Err(Error::malformed("PES packet too short"));
    }
    if payload[0..3] != [0x00, 0x00, 0x01] {
        return Err(Error::malformed("missing PES start code prefix"));
    }
    if payload[3] != stream_id {
        return Err(Error::malformed(format!(
            "PES stream_id mismatch: expected 0x{stream_id:02x}, got 0x{:02x}",
            payload[3]
        )));
    }

    let pes_len = u16::from_be_bytes([payload[4], payload[5]]);
    let mut off = 6;
    if pes_len > 0 && usize::from(pes_len) + 6 > payload.len() {
        return Err(Error::malformed("PES_packet_length exceeds payload"));
    }

    let mut pts = None;
    let mut dts = None;

    // Optional PES header: present when the next byte's top two bits are the
    // '10' marker (this byte *is* flags1 — do not consume a separate marker).
    if payload.get(off).is_some_and(|b| b & 0xc0 == 0x80) {
        let _flags1 = payload[off];
        off += 1;
        let flags2 = *payload.get(off).ok_or_else(|| Error::malformed("truncated PES flags"))?;
        off += 1;
        let pes_hdr_len = usize::from(
            *payload.get(off).ok_or_else(|| Error::malformed("truncated PES header length"))?,
        );
        off += 1;

        let pts_flag = flags2 & 0x80 != 0;
        let dts_flag = flags2 & 0x40 != 0;
        let hdr_end = off + pes_hdr_len;
        if hdr_end > payload.len() {
            return Err(Error::malformed("PES_header_data_length exceeds payload"));
        }

        let mut ts_off = off;
        if pts_flag {
            pts = Some(read_timestamp(&payload[ts_off..])?);
            ts_off += 5;
        }
        if dts_flag {
            dts = Some(read_timestamp(&payload[ts_off..])?);
        } else if pts_flag {
            dts = pts;
        }
        off = hdr_end;
    }

    Ok(PesHeader { pts, dts, data_offset: off })
}

/// Read a 33-bit MPEG timestamp field (5 bytes on wire).
fn read_timestamp(data: &[u8]) -> Result<u64> {
    if data.len() < 5 {
        return Err(Error::malformed("truncated PES timestamp"));
    }
    // The leading nibble tags the field: 0x2X = PTS-only, 0x3X = PTS (of a
    // PTS+DTS pair), 0x1X = DTS. Bit 0 is the marker bit. Accept all three;
    // rejecting 0x1X drops every frame carrying a DTS (all P/B-frames).
    let b0 = data[0];
    let marker = b0 & 0xf1;
    if marker != 0x11 && marker != 0x21 && marker != 0x31 {
        return Err(Error::malformed(format!("invalid PES timestamp marker 0x{b0:02x}")));
    }
    let val = (u64::from(b0 >> 1 & 0x07) << 30)
        | (u64::from(data[1]) << 22)
        | (u64::from(data[2] >> 1) << 15)
        | (u64::from(data[3]) << 7)
        | u64::from(data[4] >> 1);
    Ok(val)
}

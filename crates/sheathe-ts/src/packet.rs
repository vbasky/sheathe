//! MPEG-2 transport stream packet parsing (188-byte units).

use sheathe_core::{Error, Result};

/// Sync byte at the start of every transport packet.
pub const SYNC_BYTE: u8 = 0x47;
/// Standard packet size in bytes.
pub const PACKET_SIZE: usize = 188;

/// A parsed 188-byte transport packet.
#[derive(Debug, Clone, Copy)]
pub struct TsPacket<'a> {
    /// The PID this packet belongs to.
    pub pid: u16,
    /// Whether the payload begins a PES packet or PSI section.
    pub payload_unit_start: bool,
    /// Continuity counter for this PID.
    pub continuity: u8,
    /// Adaptation field bytes, if present.
    pub adaptation: Option<&'a [u8]>,
    /// The packet payload (after any adaptation field).
    pub payload: &'a [u8],
}

/// Iterate over 188-byte transport packets in `data`.
pub fn packets(data: &[u8]) -> impl Iterator<Item = Result<TsPacket<'_>>> + '_ {
    data.chunks_exact(PACKET_SIZE).map(parse_packet)
}

/// Parse one transport packet.
pub fn parse_packet(raw: &[u8]) -> Result<TsPacket<'_>> {
    if raw.len() != PACKET_SIZE {
        return Err(Error::malformed(format!(
            "transport packet is {} bytes, expected {PACKET_SIZE}",
            raw.len()
        )));
    }
    if raw[0] != SYNC_BYTE {
        return Err(Error::malformed(format!("missing sync byte 0x47, got 0x{:02x}", raw[0])));
    }

    let pid = u16::from(raw[1] & 0x1f) << 8 | u16::from(raw[2]);
    let payload_unit_start = raw[1] & 0x40 != 0;
    let continuity = raw[3] & 0x0f;
    let afc = (raw[3] >> 4) & 0x03;

    let (adaptation, payload) = match afc {
        0x00 => return Err(Error::malformed("reserved adaptation_field_control 0")),
        0x01 => (None, &raw[4..]),
        0x02 => {
            let adapt = adaptation_field(&raw[4..])?;
            (Some(adapt), &raw[4 + adapt.len()..])
        }
        0x03 => {
            let adapt = adaptation_field(&raw[4..])?;
            let off = 4 + adapt.len();
            (Some(adapt), raw.get(off..).unwrap_or(&[]))
        }
        _ => unreachable!(),
    };

    Ok(TsPacket { pid, payload_unit_start, continuity, adaptation, payload })
}

fn adaptation_field(data: &[u8]) -> Result<&[u8]> {
    let len = *data.first().ok_or_else(|| Error::malformed("truncated adaptation field"))?;
    let total = 1 + usize::from(len);
    data.get(..total).ok_or_else(|| Error::malformed("truncated adaptation field body"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_sync() {
        let raw = [0xffu8; PACKET_SIZE];
        assert!(parse_packet(&raw).is_err());
    }

    #[test]
    fn parses_payload_only_packet() {
        let mut raw = [0u8; PACKET_SIZE];
        raw[0] = SYNC_BYTE;
        raw[1] = 0x40; // payload start, pid high
        raw[2] = 0x01; // pid low = 1
        raw[3] = 0x10; // payload only
        raw[4] = 0xab;
        let pkt = parse_packet(&raw).expect("parse");
        assert_eq!(pkt.pid, 1);
        assert!(pkt.payload_unit_start);
        assert_eq!(pkt.payload[0], 0xab);
    }
}
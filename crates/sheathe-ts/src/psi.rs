//! Program-specific information: PAT and PMT section parsing.

use sheathe_core::{Error, Result};

/// Program Association Table: maps program numbers to PMT PIDs.
#[derive(Debug, Clone, Default)]
pub(crate) struct Pat {
    /// (program_number, program_map_pid). Program 0 is the NIT; real programs start at 1.
    pub programs: Vec<(u16, u16)>,
}

/// Program Map Table: maps elementary stream types to PIDs for one program.
#[derive(Debug, Clone)]
pub(crate) struct Pmt {
    /// PCR PID for this program.
    #[allow(dead_code)]
    pub(crate) pcr_pid: u16,
    /// Elementary streams: (stream_type, elementary_pid).
    pub streams: Vec<(u8, u16)>,
}

/// Parse a PAT section from `payload` (after the pointer field, if any).
pub(crate) fn parse_pat(payload: &[u8]) -> Result<Pat> {
    let body = section_body(payload, 0x00)?;
    let mut pat = Pat::default();
    // program_number(16) + reserved(3) + program_map_PID(13)
    let mut off = 8; // skip transport_stream_id(16) + reserved(2)+version(5)+current(1) + section(8)+last(8)
    while off + 4 <= body.len() {
        let program = u16::from_be_bytes([body[off], body[off + 1]]);
        let map_pid = u16::from(body[off + 2] & 0x1f) << 8 | u16::from(body[off + 3]);
        off += 4;
        if program != 0 {
            pat.programs.push((program, map_pid));
        }
    }
    if pat.programs.is_empty() {
        return Err(Error::malformed("PAT contained no programs"));
    }
    Ok(pat)
}

/// Parse a PMT section from `payload` (after the pointer field, if any).
pub(crate) fn parse_pmt(payload: &[u8]) -> Result<Pmt> {
    let body = section_body(payload, 0x02)?;
    if body.len() < 12 {
        return Err(Error::malformed("PMT section too short"));
    }
    let pcr_pid = u16::from(body[8] & 0x1f) << 8 | u16::from(body[9]);
    let prog_info_len = u16::from(body[10] & 0x0f) << 8 | u16::from(body[11]);
    let mut off = 12 + usize::from(prog_info_len);
    let mut streams = Vec::new();
    while off + 5 <= body.len() {
        let stream_type = body[off];
        let elem_pid = u16::from(body[off + 1] & 0x1f) << 8 | u16::from(body[off + 2]);
        let es_info_len = u16::from(body[off + 3] & 0x0f) << 8 | u16::from(body[off + 4]);
        off += 5 + usize::from(es_info_len);
        streams.push((stream_type, elem_pid));
    }
    if streams.is_empty() {
        return Err(Error::malformed("PMT contained no elementary streams"));
    }
    Ok(Pmt { pcr_pid, streams })
}

/// Validate the section header and return the section bytes (table_id .. CRC).
///
/// `section` must already start at the `table_id`; the caller strips the
/// `pointer_field` at the transport layer (see [`strip_pointer`]).
fn section_body(section: &[u8], expected_table_id: u8) -> Result<&[u8]> {
    if section.len() < 3 {
        return Err(Error::malformed("empty PSI section"));
    }
    if section[0] != expected_table_id {
        return Err(Error::malformed(format!(
            "expected PSI table_id 0x{expected_table_id:02x}, got 0x{:02x}",
            section[0]
        )));
    }
    let syntax = section[1] & 0x80 != 0;
    if !syntax {
        return Err(Error::malformed("PSI section_syntax_indicator not set"));
    }
    let section_length = u16::from(section[1] & 0x0f) << 8 | u16::from(section[2]);
    let total = 3 + usize::from(section_length);
    section.get(..total).ok_or_else(|| Error::malformed("truncated PSI section"))
}

/// Remove the leading pointer_field byte when present at a section start.
pub(crate) fn strip_pointer(payload: &[u8]) -> Result<(&[u8], usize)> {
    if payload.is_empty() {
        return Err(Error::malformed("empty PSI payload"));
    }
    let pointer = usize::from(payload[0]);
    let start = 1 + pointer;
    let body = payload.get(start..).ok_or_else(|| Error::malformed("invalid PSI pointer_field"))?;
    Ok((body, start))
}

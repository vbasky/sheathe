//! Extract CEA-608/708 `cc_data` triples from H.264/H.265 SEI messages.
//!
//! Both caption standards ride in `user_data_registered_itu_t_t35` (`GA94`) SEI
//! payloads as a list of `(cc_type, cc_data_1, cc_data_2)` triples. This module
//! recovers those triples (tagged with the access unit's presentation time) for
//! the 608 and 708 decoders to consume.

/// One coded-caption triple with the access unit's presentation time (ms).
pub(crate) struct Triple {
    pub pts_ms: u64,
    /// 0/1 = CEA-608 field 1/2; 2 = DTVCC continuation; 3 = DTVCC packet start.
    pub cc_type: u8,
    pub b0: u8,
    pub b1: u8,
}

/// Collect every valid `cc_data` triple across a sequence of
/// `(pts_90k, annex_b_au)` video samples.
pub(crate) fn cc_triples(samples: &[(u64, &[u8])], hevc: bool) -> Vec<Triple> {
    let mut out = Vec::new();
    for &(pts, au) in samples {
        for nal in split_nals(au) {
            if is_sei(nal, hevc) {
                collect_cc(&sei_rbsp(nal, hevc), pts / 90, &mut out);
            }
        }
    }
    out
}

/// Split Annex B data into NAL units (payloads without start codes).
fn split_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    let mut start = None;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                nals.push(&data[s..i]);
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        nals.push(&data[s..]);
    }
    nals
}

fn is_sei(nal: &[u8], hevc: bool) -> bool {
    match nal.first() {
        None => false,
        Some(&b) if hevc => ((b >> 1) & 0x3f) == 39 || ((b >> 1) & 0x3f) == 40, // PREFIX/SUFFIX SEI
        Some(&b) => (b & 0x1f) == 6,
    }
}

/// The SEI RBSP after the NAL header (emulation-prevention bytes removed).
fn sei_rbsp(nal: &[u8], hevc: bool) -> Vec<u8> {
    let header = if hevc { 2 } else { 1 };
    let mut out = Vec::with_capacity(nal.len());
    let mut i = header;
    while i < nal.len() {
        if i + 2 < nal.len() && nal[i] == 0 && nal[i + 1] == 0 && nal[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
        } else {
            out.push(nal[i]);
            i += 1;
        }
    }
    out
}

/// Walk the SEI messages in an RBSP and decode any `GA94` cc_data.
fn collect_cc(rbsp: &[u8], pts_ms: u64, out: &mut Vec<Triple>) {
    let mut i = 0;
    while i < rbsp.len() {
        // payloadType and payloadSize are 0xFF-continued.
        let mut payload_type = 0usize;
        while i < rbsp.len() && rbsp[i] == 0xff {
            payload_type += 255;
            i += 1;
        }
        if i >= rbsp.len() {
            break;
        }
        payload_type += usize::from(rbsp[i]);
        i += 1;
        let mut payload_size = 0usize;
        while i < rbsp.len() && rbsp[i] == 0xff {
            payload_size += 255;
            i += 1;
        }
        if i >= rbsp.len() {
            break;
        }
        payload_size += usize::from(rbsp[i]);
        i += 1;
        let end = (i + payload_size).min(rbsp.len());
        if payload_type == 4 {
            parse_t35(&rbsp[i..end], pts_ms, out);
        }
        i = end;
        if rbsp.get(i) == Some(&0x80) {
            break; // rbsp_trailing_bits
        }
    }
}

/// Parse a `GA94` user-data payload into cc_data triples (all types).
fn parse_t35(p: &[u8], pts_ms: u64, out: &mut Vec<Triple>) {
    // country 0xB5, provider 0x0031, user_id "GA94", type 0x03.
    if p.len() < 10 || p[0] != 0xb5 || &p[3..7] != b"GA94" || p[7] != 0x03 {
        return;
    }
    let cc_count = usize::from(p[8] & 0x1f);
    let mut idx = 10; // skip em_data byte
    for _ in 0..cc_count {
        if idx + 3 > p.len() {
            break;
        }
        let flag = p[idx];
        let cc_valid = flag & 0x04 != 0;
        let cc_type = flag & 0x03;
        let (b0, b1) = (p[idx + 1], p[idx + 2]);
        idx += 3;
        if cc_valid {
            out.push(Triple { pts_ms, cc_type, b0, b1 });
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    /// Wrap `cc_data` triples into an Annex B H.264 SEI access unit.
    /// Each triple is `(cc_type, b0, b1)`.
    pub(crate) fn sei_au(triples: &[(u8, u8, u8)]) -> Vec<u8> {
        let mut cc = vec![0xb5, 0x00, 0x31];
        cc.extend_from_slice(b"GA94");
        cc.push(0x03);
        cc.push(0xc0 | triples.len() as u8); // process_cc=1, cc_count
        cc.push(0xff); // em_data
        for &(ty, a, b) in triples {
            cc.push(0xfc | (ty & 0x03)); // marker(11111) + cc_valid(1) + cc_type(2)
            cc.push(a);
            cc.push(b);
        }
        let mut rbsp = vec![0x04, cc.len() as u8];
        rbsp.extend_from_slice(&cc);
        rbsp.push(0x80);
        let mut au = vec![0, 0, 1, 0x06];
        au.extend_from_slice(&rbsp);
        au
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::sei_au;

    #[test]
    fn extracts_all_types() {
        let au = sei_au(&[(0, 0x48, 0x49), (1, 0x41, 0x42), (3, 0x02, 0x21)]);
        let t = cc_triples(&[(90_000, au.as_slice())], false);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].cc_type, 0);
        assert_eq!((t[0].b0, t[0].b1), (0x48, 0x49));
        assert_eq!(t[1].cc_type, 1);
        assert_eq!(t[2].cc_type, 3);
        assert_eq!(t[0].pts_ms, 1000);
    }

    #[test]
    fn no_sei_no_triples() {
        let au = vec![0u8, 0, 1, 0x65, 0x88];
        assert!(cc_triples(&[(0, au.as_slice())], false).is_empty());
    }
}

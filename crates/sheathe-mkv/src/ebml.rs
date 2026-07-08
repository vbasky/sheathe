//! Minimal EBML (Extensible Binary Meta Language) element reader — the
//! container syntax underlying Matroska / WebM.

/// A cursor over EBML-encoded bytes.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Read a variable-length integer. With `keep_marker`, the leading length
    /// marker bit is retained (used for element IDs); otherwise it is stripped
    /// (used for element sizes). Returns `(value, byte_length)`.
    fn read_vint(&mut self, keep_marker: bool) -> Option<(u64, usize)> {
        let first = *self.data.get(self.pos)?;
        if first == 0 {
            return None; // 8+ byte lengths unsupported / invalid
        }
        let len = first.leading_zeros() as usize + 1; // 1..=8
        if self.pos + len > self.data.len() {
            return None;
        }
        let mut val = if keep_marker { u64::from(first) } else { u64::from(first & (0xff >> len)) };
        for i in 1..len {
            val = (val << 8) | u64::from(self.data[self.pos + i]);
        }
        self.pos += len;
        Some((val, len))
    }

    /// Read the next element at this level: `(id, body)`. An unknown or
    /// oversized declared length is clamped to the remaining buffer (so the
    /// final unknown-size element — typically the Segment — reads to EOF).
    pub(crate) fn next_element(&mut self) -> Option<(u64, &'a [u8])> {
        if self.at_end() {
            return None;
        }
        let (id, _) = self.read_vint(true)?;
        let (size, size_len) = self.read_vint(false)?;
        // All-ones size = "unknown"; also guard against corrupt oversized sizes.
        let unknown = size == (1u64 << (7 * size_len)) - 1;
        let start = self.pos;
        let end = if unknown {
            self.data.len()
        } else {
            start.saturating_add(size as usize).min(self.data.len())
        };
        self.pos = end;
        Some((id, &self.data[start..end]))
    }
}

/// Iterate the direct child elements of an EBML body.
pub(crate) fn children(body: &[u8]) -> Vec<(u64, &[u8])> {
    let mut r = Reader::new(body);
    let mut out = Vec::new();
    while let Some(el) = r.next_element() {
        out.push(el);
    }
    out
}

/// Interpret an element body as a big-endian unsigned integer.
pub(crate) fn as_uint(body: &[u8]) -> u64 {
    body.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
}

/// Interpret an element body as an IEEE-754 float (4 or 8 bytes).
pub(crate) fn as_float(body: &[u8]) -> f64 {
    match body.len() {
        4 => f32::from_be_bytes([body[0], body[1], body[2], body[3]]) as f64,
        8 => f64::from_be_bytes([
            body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7],
        ]),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_id_and_size() {
        // Element 0x83 (1-byte id), size 0x82 → 2, body [0x01,0x02].
        let data = [0x83, 0x82, 0x01, 0x02];
        let els = children(&data);
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].0, 0x83);
        assert_eq!(els[0].1, &[0x01, 0x02]);
    }

    #[test]
    fn strips_size_marker_keeps_id_marker() {
        // 4-byte id 0x1A45DFA3 (EBML header), size 0x84 → 4.
        let data = [0x1a, 0x45, 0xdf, 0xa3, 0x84, 0, 0, 0, 0];
        let els = children(&data);
        assert_eq!(els[0].0, 0x1a45dfa3);
        assert_eq!(els[0].1.len(), 4);
    }

    #[test]
    fn as_uint_and_float() {
        assert_eq!(as_uint(&[0x01, 0x00]), 256);
        assert_eq!(as_float(&48000.0f32.to_be_bytes()), 48000.0);
    }
}

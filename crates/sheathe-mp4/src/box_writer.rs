//! A minimal big-endian writer for ISO-BMFF boxes.
//!
//! Boxes are length-prefixed: a 32-bit size, a 4-byte type, then the payload.
//! [`BoxWriter::begin`]/[`BoxWriter::end`] backpatch the size once the payload
//! is known, so callers can nest boxes naturally.

/// A four-character box type code (e.g. `*b"moof"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FourCc(pub [u8; 4]);

/// Accumulates big-endian box data into a byte buffer.
#[derive(Debug, Default)]
pub struct BoxWriter {
    buf: Vec<u8>,
    /// Offsets of open boxes' size fields, for backpatching on `end`.
    open: Vec<usize>,
}

impl BoxWriter {
    /// Create an empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a box of the given type, reserving its size field.
    pub fn begin(&mut self, ty: FourCc) {
        self.open.push(self.buf.len());
        self.buf.extend_from_slice(&[0; 4]); // size placeholder
        self.buf.extend_from_slice(&ty.0);
    }

    /// Close the most recently opened box, backpatching its 32-bit size.
    pub fn end(&mut self) {
        let start = self.open.pop().expect("end() without matching begin()");
        let size = (self.buf.len() - start) as u32;
        self.buf[start..start + 4].copy_from_slice(&size.to_be_bytes());
    }

    /// Append a single byte.
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// Append a big-endian `u16`.
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Append a big-endian `u32`.
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Append a big-endian `u64`.
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Append raw bytes.
    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// The current byte length of the buffer — i.e. the offset at which the
    /// next write will land. Useful for recording a field position to backpatch.
    pub fn pos(&self) -> usize {
        self.buf.len()
    }

    /// Consume the writer and return the assembled bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        debug_assert!(self.open.is_empty(), "unterminated box on into_bytes()");
        self.buf
    }
}

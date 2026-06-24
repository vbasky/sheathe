//! A minimal, dependency-free reader for the ISO-BMFF box tree.
//!
//! Boxes are length-prefixed: a 32-bit size, a 4-byte type, then the payload.
//! A size of `1` means a 64-bit `largesize` follows the type; a size of `0`
//! means the box runs to the end of its parent. [`BoxIter`] walks one level of
//! boxes; [`Mp4Box::children`] descends into a container box.

use sheathe_core::{Error, Result};

/// One parsed box: its four-character type and its body bytes (the payload
/// after the box header, i.e. the children of a container or the fields of a
/// leaf box).
#[derive(Debug, Clone, Copy)]
pub struct Mp4Box<'a> {
    /// The four-character box type (e.g. `*b"moov"`).
    pub kind: [u8; 4],
    /// The box body: everything after the (8- or 16-byte) header.
    pub body: &'a [u8],
}

impl<'a> Mp4Box<'a> {
    /// The box type as a lossy string, for diagnostics.
    pub fn type_str(&self) -> String {
        String::from_utf8_lossy(&self.kind).into_owned()
    }

    /// Iterate the child boxes contained in this box's body.
    pub fn children(&self) -> BoxIter<'a> {
        BoxIter {
            data: self.body,
            pos: 0,
        }
    }

    /// Find the first immediate child of the given type.
    pub fn child(&self, kind: &[u8; 4]) -> Result<Option<Mp4Box<'a>>> {
        for b in self.children() {
            let b = b?;
            if &b.kind == kind {
                return Ok(Some(b));
            }
        }
        Ok(None)
    }
}

/// Iterator over a sequence of sibling boxes laid out back-to-back.
#[derive(Debug, Clone)]
pub struct BoxIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for BoxIter<'a> {
    type Item = Result<Mp4Box<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        // Stop cleanly at the end, tolerating <8 bytes of trailing padding.
        if self.data.len().saturating_sub(self.pos) < 8 {
            return None;
        }
        let start = self.pos;
        let size32 = u32::from_be_bytes(self.data[start..start + 4].try_into().unwrap());
        let kind: [u8; 4] = self.data[start + 4..start + 8].try_into().unwrap();

        let (size, header_len) = match size32 {
            // Extends to the end of the parent.
            0 => ((self.data.len() - start) as u64, 8usize),
            // 64-bit largesize follows the type.
            1 => {
                if start + 16 > self.data.len() {
                    return Some(Err(Error::malformed("truncated 64-bit box size")));
                }
                let large =
                    u64::from_be_bytes(self.data[start + 8..start + 16].try_into().unwrap());
                (large, 16usize)
            }
            n => (u64::from(n), 8usize),
        };

        if size < header_len as u64 {
            return Some(Err(Error::malformed("box size smaller than its header")));
        }
        let end = start.checked_add(size as usize);
        match end {
            Some(end) if end <= self.data.len() => {
                let body = &self.data[start + header_len..end];
                self.pos = end;
                Some(Ok(Mp4Box { kind, body }))
            }
            _ => Some(Err(Error::malformed("box extends past its parent"))),
        }
    }
}

/// Iterate the top-level boxes of a buffer (`ftyp`, `moov`, `mdat`, …).
pub fn top_level(data: &[u8]) -> BoxIter<'_> {
    BoxIter { data, pos: 0 }
}

/// A bounds-checked, big-endian cursor over a box body.
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Wrap a slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Bytes left to read.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(Error::malformed("unexpected end of box"));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// Read a `u8`.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Read a big-endian `u16`.
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    /// Read a big-endian `u32`.
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// Read a big-endian `u64`.
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// Read a four-character code.
    pub fn fourcc(&mut self) -> Result<[u8; 4]> {
        Ok(self.take(4)?.try_into().unwrap())
    }

    /// Skip `n` bytes.
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n)?;
        Ok(())
    }

    /// Read a full-box version byte + 24-bit flags, returning the version.
    pub fn version_flags(&mut self) -> Result<u8> {
        let v = self.u8()?;
        self.skip(3)?; // flags
        Ok(v)
    }
}

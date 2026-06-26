//! RBSP bit reading for H.264/H.265 parameter sets.

/// Exp-Golomb and fixed-width bit reader over RBSP bytes (emulation prevention removed).
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    pub(crate) fn read_u1(&mut self) -> Option<u32> {
        self.read_bits(1)
    }

    pub(crate) fn read_u(&mut self, n: u8) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        self.read_bits(n)
    }

    /// Read an unsigned Exp-Golomb code (ISO/IEC 14496-10 / H.265 syntax).
    pub(crate) fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zeros = 0u32;
        while self.read_u1()? == 0 {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return None;
            }
        }
        if leading_zeros == 0 {
            return Some(0);
        }
        let suffix = self.read_bits(leading_zeros as u8)?;
        let base = 1u32.checked_shl(leading_zeros)?;
        base.checked_sub(1)?.checked_add(suffix)
    }

    fn read_bits(&mut self, n: u8) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if n > 32 {
            return None;
        }
        let mut out = 0u32;
        for _ in 0..usize::from(n) {
            let byte_idx = self.bit_pos / 8;
            let bit_idx = 7 - (self.bit_pos % 8);
            let bit = (*self.data.get(byte_idx)? >> bit_idx) & 1;
            out = (out << 1) | u32::from(bit);
            self.bit_pos += 1;
        }
        Some(out)
    }
}

/// Remove H.264/H.265 emulation-prevention bytes (`0x00 0x00 0x03` → `0x00 0x00`).
pub(crate) fn rbsp_from_nal(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut i = 0;
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
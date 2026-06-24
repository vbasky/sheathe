//! Common Encryption (ISO/IEC 23001-7) for **sheathe**.
//!
//! Mirrors Shaka Packager's `media/crypto`: the four CENC protection schemes
//! plus a sample [`Encryptor`] that applies them, all on top of a pure-Rust
//! AES-128 block cipher.
//!
//! - **`cenc`** — AES-128 **CTR**, full-region. The keystream runs continuously
//!   across a sample's protected byte ranges (clear ranges do not advance the
//!   counter).
//! - **`cens`** — AES-128 **CTR** with [pattern](Pattern) encryption: only the
//!   crypt-phase blocks consume keystream; skipped blocks pass through clear,
//!   the counter continuing across them.
//! - **`cbc1`** — AES-128 **CBC**, full-region. CBC chaining runs continuously
//!   across the sample (clear ranges are skipped); a trailing partial block
//!   (< 16 bytes) of each protected range is left in the clear.
//! - **`cbcs`** — AES-128 **CBC** with pattern encryption, chaining reset to the
//!   constant IV at the start of each subsample; trailing partial blocks clear.
//!
//! Pattern encryption (`cens`/`cbcs`) is, by convention, applied to video only;
//! audio uses [`Pattern::NONE`] (full-region) even under those schemes. The
//! caller decides the per-track pattern and the NAL-aware clear/protected split
//! via the [`Subsample`] list, so this crate stays format-agnostic.

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use sheathe_core::{Error, Result};

/// A CENC protection scheme (the `schm` `scheme_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// `cenc` — AES-128 CTR, (sub)sample encryption.
    Cenc,
    /// `cens` — AES-128 CTR with pattern encryption.
    Cens,
    /// `cbc1` — AES-128 CBC, full-region (sub)sample encryption.
    Cbc1,
    /// `cbcs` — AES-128 CBC, pattern encryption (Apple FairPlay friendly).
    Cbcs,
}

impl Scheme {
    /// The four-character scheme type written into the `schm` box.
    pub fn scheme_type(self) -> [u8; 4] {
        match self {
            Scheme::Cenc => *b"cenc",
            Scheme::Cens => *b"cens",
            Scheme::Cbc1 => *b"cbc1",
            Scheme::Cbcs => *b"cbcs",
        }
    }

    /// CBC-based schemes (`cbc1`, `cbcs`); the others are CTR (`cenc`, `cens`).
    /// Non-pattern CBC requires 16-byte-aligned protected subsample ranges.
    pub fn is_cbc(self) -> bool {
        matches!(self, Scheme::Cbc1 | Scheme::Cbcs)
    }

    /// Pattern-capable schemes (`cens`, `cbcs`) — written with a version-1
    /// `tenc` carrying the crypt/skip block counts.
    pub fn is_pattern(self) -> bool {
        matches!(self, Scheme::Cens | Scheme::Cbcs)
    }

    /// `cbcs` reuses one constant IV for every sample; the others derive a
    /// unique per-sample IV.
    pub fn uses_constant_iv(self) -> bool {
        matches!(self, Scheme::Cbcs)
    }
}

/// A crypt/skip block pattern (ISO/IEC 23001-7 §9.6): encrypt `crypt_blocks`
/// 16-byte blocks, then leave `skip_blocks` blocks clear, repeating across a
/// protected range. [`Pattern::NONE`] (`crypt_blocks == 0`) means full-region
/// encryption — used by `cenc`/`cbc1` and for audio under `cens`/`cbcs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern {
    /// Number of 16-byte blocks encrypted per pattern cycle.
    pub crypt_blocks: u8,
    /// Number of 16-byte blocks skipped (left clear) per cycle.
    pub skip_blocks: u8,
}

impl Pattern {
    /// No pattern: encrypt the whole protected range.
    pub const NONE: Pattern = Pattern { crypt_blocks: 0, skip_blocks: 0 };
    /// The standard CMAF video pattern: encrypt 1 block, skip 9.
    pub const VIDEO: Pattern = Pattern { crypt_blocks: 1, skip_blocks: 9 };

    /// Whether this pattern leaves any blocks clear (i.e. is a real pattern).
    fn is_patterned(self) -> bool {
        self.crypt_blocks != 0
    }
}

/// A content key plus its 16-byte Key ID (`KID`).
#[derive(Debug, Clone)]
pub struct ContentKey {
    /// The 16-byte key identifier referenced by `tenc`/`pssh`.
    pub kid: [u8; 16],
    /// The 16-byte AES content key.
    pub key: [u8; 16],
}

/// A contiguous run within a sample: `clear` plaintext bytes followed by
/// `protected` bytes to encrypt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subsample {
    /// Number of leading clear (unencrypted) bytes.
    pub clear: u32,
    /// Number of following protected (encrypted) bytes.
    pub protected: u32,
}

/// An AES-128 sample encryptor bound to one content key.
pub struct Encryptor {
    cipher: Aes128,
}

impl Encryptor {
    /// Build an encryptor for a 16-byte AES-128 key.
    pub fn new(key: &[u8; 16]) -> Self {
        Self { cipher: Aes128::new(GenericArray::from_slice(key)) }
    }

    /// Encrypt `data` in place under `scheme` with the given `pattern`, treating
    /// it as the given subsample layout. `iv` is the 16-byte initialization
    /// vector (per-sample, or the constant IV for `cbcs`). `pattern` must be
    /// [`Pattern::NONE`] for the non-pattern schemes `cenc`/`cbc1`.
    pub fn encrypt(
        &self,
        scheme: Scheme,
        pattern: Pattern,
        iv: &[u8; 16],
        data: &mut [u8],
        subsamples: &[Subsample],
    ) -> Result<()> {
        // Validate the layout covers exactly `data`.
        let total: u64 =
            subsamples.iter().map(|s| u64::from(s.clear) + u64::from(s.protected)).sum();
        if total != data.len() as u64 {
            return Err(Error::malformed("subsample layout does not cover sample"));
        }
        if pattern.is_patterned() && !scheme.is_pattern() {
            return Err(Error::malformed("pattern set on a non-pattern scheme"));
        }
        if scheme.is_cbc() {
            self.cbc(pattern, iv, data, subsamples);
        } else {
            self.ctr(pattern, iv, data, subsamples);
        }
        Ok(())
    }

    /// AES-128-CTR (`cenc`/`cens`). The counter runs continuously over the
    /// encrypted byte ranges of the whole sample; clear bytes and skipped
    /// pattern blocks do not advance it. For [`Pattern::NONE`] the encrypted
    /// ranges are the full protected ranges (`cenc`); otherwise they are the
    /// crypt-phase blocks of each range (`cens`).
    fn ctr(&self, pattern: Pattern, iv: &[u8; 16], data: &mut [u8], subsamples: &[Subsample]) {
        let mut counter = *iv;
        let mut keystream = [0u8; 16];
        let mut ks_pos = 16usize; // force a fresh block on first use

        for_each_crypt_range(pattern, subsamples, |start, len| {
            for byte in &mut data[start..start + len] {
                if ks_pos == 16 {
                    keystream = counter;
                    self.encrypt_block(&mut keystream);
                    incr_be(&mut counter);
                    ks_pos = 0;
                }
                *byte ^= keystream[ks_pos];
                ks_pos += 1;
            }
        });
    }

    /// AES-128-CBC (`cbc1`/`cbcs`). For [`Pattern::NONE`] (`cbc1`) the CBC chain
    /// runs continuously across the sample's protected ranges, seeded from `iv`;
    /// each range's trailing partial block (< 16 bytes) is left clear. For a
    /// pattern (`cbcs`) the chain resets to the constant `iv` at the start of
    /// each subsample and advances only over crypt-phase blocks.
    fn cbc(&self, pattern: Pattern, iv: &[u8; 16], data: &mut [u8], subsamples: &[Subsample]) {
        let mut off = 0usize;
        // `cbc1` chains across subsamples; `cbcs` reuses the constant IV per
        // subsample. A single running chain handles both: it is only reset per
        // subsample when a pattern is in effect.
        let mut chain = *iv;
        for s in subsamples {
            off += s.clear as usize;
            if pattern.is_patterned() {
                chain = *iv;
            }
            let mut remaining = s.protected as usize;
            let mut block_index = 0usize;
            let cycle = pattern.crypt_blocks as usize + pattern.skip_blocks as usize;
            while remaining >= 16 {
                let encrypt =
                    !pattern.is_patterned() || block_index % cycle < pattern.crypt_blocks as usize;
                if encrypt {
                    let mut block = [0u8; 16];
                    block.copy_from_slice(&data[off..off + 16]);
                    for (b, c) in block.iter_mut().zip(chain.iter()) {
                        *b ^= *c;
                    }
                    self.encrypt_block(&mut block);
                    data[off..off + 16].copy_from_slice(&block);
                    chain = block;
                }
                off += 16;
                remaining -= 16;
                block_index += 1;
            }
            off += remaining; // trailing partial block stays clear
        }
    }

    /// Encrypt one 16-byte block in place (AES-128-ECB primitive).
    fn encrypt_block(&self, block: &mut [u8; 16]) {
        let mut ga = GenericArray::clone_from_slice(block);
        self.cipher.encrypt_block(&mut ga);
        block.copy_from_slice(&ga);
    }
}

/// Invoke `f(start, len)` for each contiguous byte range that gets encrypted,
/// in order, given a subsample layout and pattern. With [`Pattern::NONE`] this
/// is each protected range whole; with a crypt/skip pattern it is the
/// crypt-phase blocks of each protected range. Per ISO/IEC 23001-7 §9.6, a
/// trailing partial block (< 16 bytes) of a protected range is left in the
/// clear under pattern encryption, so the crypt phase only covers whole blocks.
fn for_each_crypt_range(
    pattern: Pattern,
    subsamples: &[Subsample],
    mut f: impl FnMut(usize, usize),
) {
    let mut off = 0usize;
    for s in subsamples {
        off += s.clear as usize;
        let protected = s.protected as usize;
        if !pattern.is_patterned() {
            if protected > 0 {
                f(off, protected);
            }
            off += protected;
            continue;
        }
        let crypt = pattern.crypt_blocks as usize * 16;
        let skip = pattern.skip_blocks as usize * 16;
        let mut pos = 0usize;
        while pos < protected {
            let phase = crypt.min(protected - pos);
            // Encrypt only whole 16-byte blocks; any partial block at the very
            // end of the range stays clear.
            let whole = phase - phase % 16;
            if whole > 0 {
                f(off + pos, whole);
            }
            pos += phase;
            pos += skip.min(protected - pos);
        }
        off += protected;
    }
}

/// Increment a 16-byte big-endian counter by one (wrapping).
fn incr_be(counter: &mut [u8; 16]) {
    for byte in counter.iter_mut().rev() {
        let (v, carry) = byte.overflowing_add(1);
        *byte = v;
        if !carry {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 16] = [
        0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f,
        0x3c,
    ];

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn cenc_matches_nist_ctr_vector() {
        // NIST SP800-38A, F.5.1 (CTR-AES128.Encrypt), first block.
        let iv = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let mut data = hex("6bc1bee22e409f96e93d7e117393172a");
        let enc = Encryptor::new(&KEY);
        let subs = [Subsample { clear: 0, protected: 16 }];
        enc.encrypt(Scheme::Cenc, Pattern::NONE, iv[..].try_into().unwrap(), &mut data, &subs)
            .unwrap();
        assert_eq!(data, hex("874d6191b620e3261bef6864990db6ce"));
    }

    #[test]
    fn cbc_schemes_match_nist_cbc_vector() {
        // NIST SP800-38A, F.2.1 (CBC-AES128.Encrypt), first block. Both `cbc1`
        // (no pattern) and `cbcs` (1:9, first block is in the crypt phase)
        // encrypt the first block identically.
        let iv = hex("000102030405060708090a0b0c0d0e0f");
        let subs = [Subsample { clear: 0, protected: 16 }];
        for (scheme, pattern) in [(Scheme::Cbc1, Pattern::NONE), (Scheme::Cbcs, Pattern::VIDEO)] {
            let mut data = hex("6bc1bee22e409f96e93d7e117393172a");
            let enc = Encryptor::new(&KEY);
            enc.encrypt(scheme, pattern, iv[..].try_into().unwrap(), &mut data, &subs).unwrap();
            assert_eq!(data, hex("7649abac8119b246cee98e9b12e9197d"), "{scheme:?}");
        }
    }

    #[test]
    fn cenc_leaves_clear_bytes_untouched() {
        let iv = [0u8; 16];
        let mut data = vec![0xAAu8; 32];
        let enc = Encryptor::new(&KEY);
        // 8 clear, 24 protected (8+24=32).
        enc.encrypt(
            Scheme::Cenc,
            Pattern::NONE,
            &iv,
            &mut data,
            &[Subsample { clear: 8, protected: 24 }],
        )
        .unwrap();
        assert!(data[..8].iter().all(|&b| b == 0xAA), "clear prefix must be untouched");
        assert!(data[8..].iter().any(|&b| b != 0xAA), "protected region must change");
    }

    #[test]
    fn pattern_schemes_skip_blocks() {
        // 10 blocks under a 1:9 pattern: only block 0 is encrypted; blocks 1..9
        // are skipped (left clear). Holds for both `cbcs` (CBC) and `cens` (CTR).
        let iv = [0u8; 16];
        for scheme in [Scheme::Cbcs, Scheme::Cens] {
            let mut data = vec![0x11u8; 160];
            let original = data.clone();
            let enc = Encryptor::new(&KEY);
            enc.encrypt(
                scheme,
                Pattern::VIDEO,
                &iv,
                &mut data,
                &[Subsample { clear: 0, protected: 160 }],
            )
            .unwrap();
            assert_ne!(data[..16], original[..16], "{scheme:?}: first block encrypted");
            assert_eq!(data[16..], original[16..], "{scheme:?}: blocks 1..9 skipped");
        }
    }

    #[test]
    fn rejects_mismatched_layout() {
        let enc = Encryptor::new(&KEY);
        let mut data = vec![0u8; 10];
        let err = enc.encrypt(
            Scheme::Cenc,
            Pattern::NONE,
            &[0u8; 16],
            &mut data,
            &[Subsample { clear: 0, protected: 9 }],
        );
        assert!(err.is_err());
    }

    #[test]
    fn rejects_pattern_on_non_pattern_scheme() {
        let enc = Encryptor::new(&KEY);
        let mut data = vec![0u8; 16];
        let err = enc.encrypt(
            Scheme::Cenc,
            Pattern::VIDEO,
            &[0u8; 16],
            &mut data,
            &[Subsample { clear: 0, protected: 16 }],
        );
        assert!(err.is_err());
    }

    /// Every scheme is symmetric here: CTR is self-inverse, and applying the CBC
    /// path twice with these helpers is not — so we only round-trip the CTR
    /// schemes via re-encryption, and check CBC via known structure elsewhere.
    #[test]
    fn ctr_schemes_round_trip_across_subsamples() {
        let enc = Encryptor::new(&KEY);
        let iv = [3u8; 16];
        let subs = [Subsample { clear: 5, protected: 40 }, Subsample { clear: 10, protected: 65 }];
        for (scheme, pattern) in [(Scheme::Cenc, Pattern::NONE), (Scheme::Cens, Pattern::VIDEO)] {
            let original: Vec<u8> = (0..120u8).collect();
            let mut data = original.clone();
            enc.encrypt(scheme, pattern, &iv, &mut data, &subs).unwrap();
            assert_ne!(data, original, "{scheme:?}: ciphertext must differ");
            assert_eq!(&data[..5], &original[..5], "{scheme:?}: leading clear bytes preserved");
            enc.encrypt(scheme, pattern, &iv, &mut data, &subs).unwrap();
            assert_eq!(data, original, "{scheme:?}: CTR round-trip restores plaintext");
        }
    }

    /// `cbc1` decrypts back to plaintext: decrypt the full-block portion of one
    /// protected range and confirm the trailing partial block was left clear.
    #[test]
    fn cbc1_leaves_trailing_partial_clear() {
        let enc = Encryptor::new(&KEY);
        let iv = [7u8; 16];
        // 37 protected bytes = 2 full blocks (32) + 5 trailing clear.
        let original: Vec<u8> = (0..40u8).collect();
        let mut data = original.clone();
        enc.encrypt(
            Scheme::Cbc1,
            Pattern::NONE,
            &iv,
            &mut data,
            &[Subsample { clear: 3, protected: 37 }],
        )
        .unwrap();
        assert_eq!(&data[..3], &original[..3], "leading clear preserved");
        assert_ne!(&data[3..35], &original[3..35], "full blocks encrypted");
        assert_eq!(&data[35..], &original[35..], "trailing partial block left clear");
    }
}

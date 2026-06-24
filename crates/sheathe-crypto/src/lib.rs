//! Common Encryption (ISO/IEC 23001-7) for **sheathe**.
//!
//! Mirrors Shaka Packager's `media/crypto`: the CENC protection schemes plus a
//! sample [`Encryptor`] that applies them. Two schemes are implemented on top of
//! a pure-Rust AES-128 block cipher:
//!
//! - **`cenc`** — AES-128 **CTR**. The keystream runs continuously across a
//!   sample's protected byte ranges (clear ranges do not advance the counter).
//! - **`cbcs`** — AES-128 **CBC** with pattern encryption (crypt 1 block, skip
//!   9), CBC chaining reset to the constant IV at the start of each subsample;
//!   a trailing partial block (< 16 bytes) is left in the clear.
//!
//! Encryption operates on a list of [`Subsample`] (clear/protected byte runs),
//! so the caller (the MP4 muxer) decides the NAL-aware clear/protected split;
//! this crate stays format-agnostic.

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use sheathe_core::{Error, Result};

/// A CENC protection scheme (the `schm` `scheme_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// `cenc` — AES-128 CTR, (sub)sample encryption.
    Cenc,
    /// `cbcs` — AES-128 CBC, pattern encryption (Apple FairPlay friendly).
    Cbcs,
}

impl Scheme {
    /// The four-character scheme type written into the `schm` box.
    pub fn scheme_type(self) -> [u8; 4] {
        match self {
            Scheme::Cenc => *b"cenc",
            Scheme::Cbcs => *b"cbcs",
        }
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

/// `cbcs` pattern: encrypt 1 of every 10 sixteen-byte blocks.
const CBCS_CRYPT_BLOCKS: usize = 1;
const CBCS_PATTERN_BLOCKS: usize = 10;

/// An AES-128 sample encryptor bound to one content key.
pub struct Encryptor {
    cipher: Aes128,
}

impl Encryptor {
    /// Build an encryptor for a 16-byte AES-128 key.
    pub fn new(key: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(GenericArray::from_slice(key)),
        }
    }

    /// Encrypt `data` in place under `scheme`, treating it as the given
    /// subsample layout. `iv` is the 16-byte per-sample initialization vector.
    pub fn encrypt(
        &self,
        scheme: Scheme,
        iv: &[u8; 16],
        data: &mut [u8],
        subsamples: &[Subsample],
    ) -> Result<()> {
        // Validate the layout covers exactly `data`.
        let total: u64 = subsamples
            .iter()
            .map(|s| u64::from(s.clear) + u64::from(s.protected))
            .sum();
        if total != data.len() as u64 {
            return Err(Error::malformed("subsample layout does not cover sample"));
        }
        match scheme {
            Scheme::Cenc => self.cenc(iv, data, subsamples),
            Scheme::Cbcs => self.cbcs(iv, data, subsamples),
        }
        Ok(())
    }

    /// AES-128-CTR with a counter continuous across protected bytes.
    fn cenc(&self, iv: &[u8; 16], data: &mut [u8], subsamples: &[Subsample]) {
        let mut counter = *iv;
        let mut keystream = [0u8; 16];
        let mut ks_pos = 16usize; // force a fresh block on first use
        let mut off = 0usize;

        for s in subsamples {
            off += s.clear as usize;
            let end = off + s.protected as usize;
            while off < end {
                if ks_pos == 16 {
                    keystream = counter;
                    self.encrypt_block(&mut keystream);
                    incr_be(&mut counter);
                    ks_pos = 0;
                }
                data[off] ^= keystream[ks_pos];
                ks_pos += 1;
                off += 1;
            }
        }
    }

    /// AES-128-CBC with 1:9 pattern encryption per subsample.
    fn cbcs(&self, iv: &[u8; 16], data: &mut [u8], subsamples: &[Subsample]) {
        let mut off = 0usize;
        for s in subsamples {
            off += s.clear as usize;
            let mut remaining = s.protected as usize;
            let mut chain = *iv;
            let mut block_index = 0usize;
            while remaining >= 16 {
                if block_index % CBCS_PATTERN_BLOCKS < CBCS_CRYPT_BLOCKS {
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
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cenc_matches_nist_ctr_vector() {
        // NIST SP800-38A, F.5.1 (CTR-AES128.Encrypt), first block.
        let iv = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let mut data = hex("6bc1bee22e409f96e93d7e117393172a");
        let enc = Encryptor::new(&KEY);
        let subs = [Subsample {
            clear: 0,
            protected: 16,
        }];
        enc.encrypt(Scheme::Cenc, iv[..].try_into().unwrap(), &mut data, &subs)
            .unwrap();
        assert_eq!(data, hex("874d6191b620e3261bef6864990db6ce"));
    }

    #[test]
    fn cbcs_first_block_matches_nist_cbc_vector() {
        // NIST SP800-38A, F.2.1 (CBC-AES128.Encrypt), first block.
        let iv = hex("000102030405060708090a0b0c0d0e0f");
        let mut data = hex("6bc1bee22e409f96e93d7e117393172a");
        let enc = Encryptor::new(&KEY);
        let subs = [Subsample {
            clear: 0,
            protected: 16,
        }];
        enc.encrypt(Scheme::Cbcs, iv[..].try_into().unwrap(), &mut data, &subs)
            .unwrap();
        assert_eq!(data, hex("7649abac8119b246cee98e9b12e9197d"));
    }

    #[test]
    fn cenc_leaves_clear_bytes_untouched() {
        let iv = [0u8; 16];
        let mut data = vec![0xAAu8; 32];
        let enc = Encryptor::new(&KEY);
        // 8 clear, 24 protected (8+24=32).
        enc.encrypt(
            Scheme::Cenc,
            &iv,
            &mut data,
            &[Subsample {
                clear: 8,
                protected: 24,
            }],
        )
        .unwrap();
        assert!(
            data[..8].iter().all(|&b| b == 0xAA),
            "clear prefix must be untouched"
        );
        assert!(
            data[8..].iter().any(|&b| b != 0xAA),
            "protected region must change"
        );
    }

    #[test]
    fn cbcs_pattern_skips_blocks() {
        let iv = [0u8; 16];
        // 10 blocks: only block 0 is encrypted, blocks 1..9 skipped (clear).
        let mut data = vec![0x11u8; 160];
        let original = data.clone();
        let enc = Encryptor::new(&KEY);
        enc.encrypt(
            Scheme::Cbcs,
            &iv,
            &mut data,
            &[Subsample {
                clear: 0,
                protected: 160,
            }],
        )
        .unwrap();
        assert_ne!(data[..16], original[..16], "first block encrypted");
        assert_eq!(data[16..], original[16..], "blocks 1..9 skipped");
    }

    #[test]
    fn rejects_mismatched_layout() {
        let enc = Encryptor::new(&KEY);
        let mut data = vec![0u8; 10];
        let err = enc.encrypt(
            Scheme::Cenc,
            &[0u8; 16],
            &mut data,
            &[Subsample {
                clear: 0,
                protected: 9,
            }],
        );
        assert!(err.is_err());
    }

    #[test]
    fn cenc_round_trips_across_subsamples() {
        // CTR is symmetric: encrypting the ciphertext again recovers the input,
        // exercising the continuous counter across multiple subsamples and the
        // preservation of clear regions.
        let enc = Encryptor::new(&KEY);
        let iv = [3u8; 16];
        let subs = [
            Subsample {
                clear: 5,
                protected: 20,
            },
            Subsample {
                clear: 10,
                protected: 65,
            },
        ];
        let original: Vec<u8> = (0..100u8).collect();
        let mut data = original.clone();

        enc.encrypt(Scheme::Cenc, &iv, &mut data, &subs).unwrap();
        assert_ne!(data, original, "ciphertext must differ");
        assert_eq!(&data[..5], &original[..5], "leading clear bytes preserved");

        enc.encrypt(Scheme::Cenc, &iv, &mut data, &subs).unwrap();
        assert_eq!(data, original, "CTR round-trip restores plaintext");
    }
}

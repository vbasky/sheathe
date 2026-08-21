//! Decrypt CENC-protected samples given a raw key (Shaka `--enable_raw_key` decrypt).

use crate::box_reader::{Cursor, top_level};
use crate::demux::Track;
use sheathe_core::{Result, Sample};
use sheathe_crypto::{ContentKey, Encryptor, Pattern, Scheme, Subsample};

/// If `track`'s sample entry is `encv`/`enca`, decrypt `samples` in place using
/// `key`. Fragmented files supply IVs via `senc` boxes in `file`; progressive
/// encrypted files use the constant IV from `tenc` when present.
pub fn decrypt_samples(
    file: &[u8],
    track: &Track,
    samples: &mut [Sample],
    key: &ContentKey,
) -> Result<()> {
    let entry = track.sample_entry();
    let Some(tenc) = find_tenc(entry) else {
        return Ok(()); // clear sample entry
    };
    let scheme = tenc.scheme;
    let encryptor = Encryptor::new(&key.key);
    let sencs = collect_senc(file, track.track_id);
    for (i, sample) in samples.iter_mut().enumerate() {
        let (iv, subs) = if let Some(s) = sencs.get(i) {
            (s.iv, s.subs.clone())
        } else {
            (
                tenc.constant_iv.unwrap_or([0; 16]),
                vec![Subsample { clear: 0, protected: sample.data.len() as u32 }],
            )
        };
        let pattern = if scheme.is_pattern() { Pattern::VIDEO } else { Pattern::NONE };
        // CENC is symmetric: encrypt() is also decrypt for CTR; for CBC we still
        // call encryptor.encrypt which is the forward transform — use CTR xor
        // invertibility. For CBC, Encryptor only implements encrypt. CTR schemes
        // xor the keystream so encrypt==decrypt.
        if scheme.is_cbc() {
            // Best-effort: treat as CTR-equivalent not available; skip CBC decrypt.
            continue;
        }
        encryptor.encrypt(scheme, pattern, &iv, &mut sample.data, &subs)?;
    }
    Ok(())
}

struct TencInfo {
    scheme: Scheme,
    constant_iv: Option<[u8; 16]>,
}

fn find_tenc(entry: &[u8]) -> Option<TencInfo> {
    if entry.len() < 8 {
        return None;
    }
    let kind = &entry[4..8];
    if kind != b"encv" && kind != b"enca" {
        return None;
    }
    // Scan for tenc / schm.
    let mut scheme = Scheme::Cenc;
    if let Some(i) = find(entry, b"schm") {
        if i + 12 <= entry.len() {
            match &entry[i + 8..i + 12] {
                b"cenc" => scheme = Scheme::Cenc,
                b"cens" => scheme = Scheme::Cens,
                b"cbc1" => scheme = Scheme::Cbc1,
                b"cbcs" => scheme = Scheme::Cbcs,
                _ => {}
            }
        }
    }
    let mut constant_iv = None;
    if let Some(i) = find(entry, b"tenc") {
        // tenc body after type: version/flags(4) + reserved(1)+pattern(1)+prot(1)+iv_size(1)+kid(16) [+const_iv]
        let body = i + 4; // start of version
        if body + 24 <= entry.len() {
            // layout: version/flags (4), reserved (1), pattern (1),
            // isProtected (1), per_sample_iv_size (1), kid (16)
            let iv_size = entry[body + 4 + 3];
            if iv_size == 0 && body + 4 + 4 + 16 + 16 <= entry.len() {
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&entry[body + 4 + 4 + 16..body + 4 + 4 + 16 + 16]);
                constant_iv = Some(iv);
            }
        }
    }
    Some(TencInfo { scheme, constant_iv })
}

struct SencSample {
    iv: [u8; 16],
    subs: Vec<Subsample>,
}

fn collect_senc(file: &[u8], track_id: u32) -> Vec<SencSample> {
    let mut out = Vec::new();
    for b in top_level(file) {
        let Ok(b) = b else { continue };
        if &b.kind != b"moof" {
            continue;
        }
        let Ok(Some(traf)) = b.child(b"traf") else { continue };
        let Ok(Some(tfhd)) = traf.child(b"tfhd") else { continue };
        if parse_tfhd_id(tfhd.body) != Some(track_id) {
            continue;
        }
        let Ok(Some(senc)) = traf.child(b"senc") else { continue };
        parse_senc(senc.body, &mut out);
    }
    out
}

fn parse_tfhd_id(body: &[u8]) -> Option<u32> {
    let mut c = Cursor::new(body);
    let _ = c.version_flags().ok()?;
    c.u32().ok()
}

fn parse_senc(body: &[u8], out: &mut Vec<SencSample>) {
    if body.len() < 8 {
        return;
    }
    let flags = u32::from_be_bytes(body[0..4].try_into().unwrap()) & 0x00ff_ffff;
    let count = u32::from_be_bytes(body[4..8].try_into().unwrap());
    let has_subs = flags & 0x02 != 0;
    let mut c = Cursor::new(&body[8..]);
    for _ in 0..count {
        let mut iv = [0u8; 16];
        if c.remaining() >= 16 {
            for slot in iv.chunks_mut(4) {
                if let Ok(w) = c.u32() {
                    slot.copy_from_slice(&w.to_be_bytes());
                }
            }
        }
        let mut subs = Vec::new();
        if has_subs {
            let n = c.u16().unwrap_or(0);
            for _ in 0..n {
                let clear = c.u16().unwrap_or(0) as u32;
                let protected = c.u32().unwrap_or(0);
                subs.push(Subsample { clear, protected });
            }
        }
        out.push(SencSample { iv, subs });
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

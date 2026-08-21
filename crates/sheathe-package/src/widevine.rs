//! Widevine Common Encryption key-server client (Shaka `--enable_widevine_encryption`).
//!
//! Request format matches Shaka Packager: protobuf-JSON `CommonEncryptionRequest`
//! wrapped in `SignedModularDrmRequest`, signed with AES-CBC(SHA-1(request)) or
//! RSA PKCS#1 v1.5 SHA-1.

use crate::knobs::{LabeledKey, WidevineConfig};
use crate::tls::{HttpOptions, http_post};
use anyhow::{Context, Result, bail};
use sha1::{Digest, Sha1};
use sheathe_crypto::aes_cbc_pkcs7_encrypt;

/// Fetch content keys from a Widevine CENC key server.
pub(crate) fn fetch_keys(
    cfg: &WidevineConfig,
    http: &HttpOptions,
    protection_scheme: &str,
) -> Result<Vec<LabeledKey>> {
    let inner = build_request_json(cfg, protection_scheme);
    let signature = sign(&inner, cfg)?;
    let sig_b64 = b64(&signature);
    let outer = format!(
        "{{\"request\":{},\"signature\":\"{}\",\"signer\":\"{}\"}}",
        json_string(&inner),
        sig_b64,
        json_escape(&cfg.signer),
    );
    let resp = http_post(&cfg.key_server_url, outer.as_bytes(), "application/json", http, &[])?;
    parse_response(&String::from_utf8_lossy(&resp))
}

fn build_request_json(cfg: &WidevineConfig, scheme: &str) -> String {
    let scheme_enum = match scheme {
        "cbcs" => "CBCS",
        "cbc1" => "CBC1",
        "cens" => "CENS",
        _ => "CENC",
    };
    let mut s = String::from("{");
    s.push_str(&format!("\"content_id\":\"{}\"", b64(&cfg.content_id)));
    if !cfg.policy.is_empty() {
        s.push_str(&format!(",\"policy\":\"{}\"", json_escape(&cfg.policy)));
    }
    s.push_str(&format!(",\"protection_scheme\":\"{scheme_enum}\""));
    s.push_str(",\"tracks\":[{\"type\":\"SD\"},{\"type\":\"HD\"},{\"type\":\"UHD1\"},{\"type\":\"UHD2\"},{\"type\":\"AUDIO\"}]");
    s.push_str(",\"drm_types\":[\"WIDEVINE\"]");
    if cfg.enable_entitlement_license {
        s.push_str(",\"enable_entitlement_license\":true");
    }
    if let Some(g) = &cfg.group_id {
        s.push_str(&format!(",\"group_id\":\"{}\"", b64(g)));
    }
    s.push('}');
    s
}

fn sign(message: &str, cfg: &WidevineConfig) -> Result<Vec<u8>> {
    if let (Some(key), Some(iv)) = (&cfg.aes_signing_key, &cfg.aes_signing_iv) {
        let mut hasher = Sha1::new();
        hasher.update(message.as_bytes());
        let hash = hasher.finalize();
        return aes_cbc_pkcs7_encrypt(key, iv, &hash).context("AES signing");
    }
    if let Some(pem) = &cfg.rsa_signing_key_pem {
        return rsa_sign(message.as_bytes(), pem);
    }
    bail!("Widevine signing requires --aes-signing-key/--aes-signing-iv or --rsa-signing-key-path");
}

fn rsa_sign(message: &[u8], pem: &str) -> Result<Vec<u8>> {
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::DecodePrivateKey;
    use rsa::signature::{SignatureEncoding, Signer};
    let key = rsa::RsaPrivateKey::from_pkcs1_pem(pem)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs8_pem(pem))
        .context("parsing RSA signing key")?;
    let signing: SigningKey<Sha1> = SigningKey::<Sha1>::new_unprefixed(key);
    Ok(signing.sign(message).to_vec())
}

fn parse_response(json: &str) -> Result<Vec<LabeledKey>> {
    // Nested JSON: { "response": "{ ... CommonEncryptionResponse ... }" }
    let inner = json_field(json, "response").unwrap_or(json);
    let inner = unquote_json_string(inner);
    let status = json_field(&inner, "status").map(unquote).unwrap_or_else(|| "OK".into());
    anyhow::ensure!(status == "OK" || status == "0", "Widevine status {status}: {inner}");

    let mut keys = Vec::new();
    let tracks = json_array(&inner, "tracks").unwrap_or_default();
    for t in tracks {
        let ty = json_field(&t, "type").unwrap_or("SD");
        let key_b64 = json_field(&t, "key").context("track.key")?;
        let kid_b64 = json_field(&t, "key_id").or_else(|| json_field(&t, "keyId"));
        let key = b64_decode(&unquote(key_b64))?;
        let kid = if let Some(k) = kid_b64 {
            let v = b64_decode(&unquote(k))?;
            v.try_into().ok()
        } else {
            None
        };
        anyhow::ensure!(key.len() == 16, "Widevine key length {}", key.len());
        let kid = kid.unwrap_or([0; 16]);
        keys.push(LabeledKey { label: unquote(ty), kid, key: key.try_into().unwrap(), iv: None });
    }
    anyhow::ensure!(!keys.is_empty(), "Widevine response contained no tracks");
    Ok(keys)
}

fn json_string(s: &str) -> String {
    format!("\"{}\"", json_escape(s))
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

fn json_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\"");
    let i = json.find(&pat)?;
    let rest = json[i + pat.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if rest.starts_with('"') {
        let inner = rest.strip_prefix('"').unwrap_or(rest);
        let mut j = 0;
        let b = inner.as_bytes();
        while j < b.len() {
            if b[j] == b'\\' {
                j += 2;
                continue;
            }
            if b[j] == b'"' {
                return Some(&rest[..j + 2]);
            }
            j += 1;
        }
        None
    } else {
        let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

fn json_array(json: &str, key: &str) -> Option<Vec<String>> {
    let pat = format!("\"{key}\"");
    let i = json.find(&pat)?;
    let rest = json[i + pat.len()..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('[')?;
    let mut items = Vec::new();
    let mut depth = 1i32;
    let mut start = 0usize;
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() && depth > 0 {
        match b[i] {
            b'{' => {
                if depth == 1 {
                    start = i;
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 1 {
                    items.push(rest[start..=i].to_string());
                }
            }
            b']' if depth == 1 => break,
            _ => {}
        }
        i += 1;
    }
    Some(items)
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        json_unescape(&t[1..t.len() - 1])
    } else {
        t.to_string()
    }
}

fn unquote_json_string(s: &str) -> String {
    unquote(s)
}

fn json_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(A[(n >> 18 & 0x3f) as usize] as char);
        out.push(A[(n >> 12 & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6 & 0x3f) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[(n & 0x3f) as usize] as char } else { '=' });
    }
    out
}

pub(crate) fn b64_decode_pub(s: &str) -> Result<Vec<u8>> {
    b64_decode(s)
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').collect();
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let a = val(chunk[0]).context("b64")?;
        let b = val(*chunk.get(1).unwrap_or(&b'A')).context("b64")?;
        let c = val(*chunk.get(2).unwrap_or(&b'A')).unwrap_or(0);
        let d = val(*chunk.get(3).unwrap_or(&b'A')).unwrap_or(0);
        let n = (u32::from(a) << 18) | (u32::from(b) << 12) | (u32::from(c) << 6) | u32::from(d);
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_sign_is_32_bytes() {
        let cfg = WidevineConfig {
            key_server_url: String::new(),
            content_id: b"test".to_vec(),
            signer: "widevine_test".into(),
            aes_signing_key: Some(vec![0x11; 32]),
            aes_signing_iv: Some(vec![0x22; 16]),
            rsa_signing_key_pem: None,
            policy: String::new(),
            group_id: None,
            enable_entitlement_license: false,
            decrypt: false,
        };
        let sig = sign("hello", &cfg).unwrap();
        assert_eq!(sig.len(), 32); // SHA-1 (20) + PKCS7 → 32
    }

    #[test]
    fn parse_tracks() {
        let json = r#"{"status":"OK","tracks":[{"type":"SD","key":"AAECAwQFBgcICQoLDA0ODw==","key_id":"AAECAwQFBgcICQoLDA0ODw=="}]}"#;
        let keys = parse_response(json).unwrap();
        assert_eq!(keys[0].label, "SD");
        assert_eq!(keys[0].key[0], 0);
        assert_eq!(keys[0].key[1], 1);
    }
}

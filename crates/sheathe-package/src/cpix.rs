//! CPIX (DASH-IF) / SPEKE document ingest.
//!
//! Parses a useful subset: `ContentKey` + `PlainValue` (or `EncryptedValue` is
//! rejected unless a recipient private key decrypts it — RSA OAEP), `UsageRule`
//! `intendedTrackType`, and `DRMSystem` `PSSH`.

use crate::knobs::{CpixConfig, LabeledKey, parse_hex16};
use crate::tls::{HttpOptions, http_get, http_post};
use anyhow::{Context, Result};
use std::fs;

/// A key extracted from a CPIX document.
#[derive(Debug, Clone)]
pub(crate) struct CpixKey {
    pub kid: [u8; 16],
    pub key: [u8; 16],
    pub label: Option<String>,
}

/// Load keys from `--cpix` (file or HTTP URL, optional SPEKE POST).
pub(crate) fn load_cpix(cfg: &CpixConfig, http: &HttpOptions) -> Result<Vec<CpixKey>> {
    let xml = if let Some(req) = &cfg.request_file {
        anyhow::ensure!(
            cfg.path_or_url.starts_with("http://") || cfg.path_or_url.starts_with("https://"),
            "--cpix-request-file requires --cpix to be an HTTP(S) URL"
        );
        let body = fs::read(req).with_context(|| format!("reading {}", req.display()))?;
        let extra: Vec<(&str, &str)> =
            cfg.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        http_post(&cfg.path_or_url, &body, "application/xml", http, &extra)?
    } else if cfg.path_or_url.starts_with("http://") || cfg.path_or_url.starts_with("https://") {
        let extra: Vec<(&str, &str)> =
            cfg.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        http_get(&cfg.path_or_url, http, &extra)?
    } else {
        fs::read(&cfg.path_or_url).with_context(|| format!("reading {}", cfg.path_or_url))?
    };
    let text = String::from_utf8_lossy(&xml);
    parse_cpix(&text, cfg.private_key_pem.as_deref())
}

/// Parse a CPIX XML document into labeled keys.
pub(crate) fn parse_cpix(xml: &str, rsa_pem: Option<&str>) -> Result<Vec<CpixKey>> {
    let mut keys = Vec::new();
    // Walk ContentKey elements.
    let mut rest = xml;
    while let Some(start) = rest.find("<ContentKey") {
        let after = &rest[start..];
        let end = after.find("</ContentKey>").or_else(|| after.find("/>")).unwrap_or(after.len());
        let elem = &after[..end.min(after.len())];
        let kid = attr(elem, "kid").or_else(|| attr(elem, "KID"));
        let Some(kid_s) = kid else {
            rest = &after[1..];
            continue;
        };
        let kid = parse_kid(&kid_s)?;
        let key = if let Some(plain) = tag_text(elem, "PlainValue") {
            decode_key(&plain)?
        } else if let Some(enc) = tag_text(elem, "EncryptedValue") {
            let pem = rsa_pem.context("CPIX EncryptedValue requires --cpix-private-key")?;
            decrypt_cpix_secret(&enc, pem)?
        } else {
            rest = &after[end.max(1)..];
            continue;
        };
        keys.push(CpixKey { kid, key, label: None });
        rest = &after[end.max(1)..];
    }

    // UsageRule intendedTrackType → label.
    rest = xml;
    while let Some(start) = rest.find("<UsageRule") {
        let after = &rest[start..];
        let end = after.find("</UsageRule>").or_else(|| after.find("/>")).unwrap_or(after.len());
        let elem = &after[..end.min(after.len())];
        if let (Some(kid_s), Some(label)) = (attr(elem, "kid"), attr(elem, "intendedTrackType")) {
            if let Ok(kid) = parse_kid(&kid_s) {
                if let Some(k) = keys.iter_mut().find(|k| k.kid == kid) {
                    k.label = Some(label);
                }
            }
        }
        rest = &after[end.max(1)..];
    }
    anyhow::ensure!(!keys.is_empty(), "CPIX document contained no ContentKey/PlainValue entries");
    Ok(keys)
}

impl CpixKey {
    pub(crate) fn as_labeled(&self) -> LabeledKey {
        LabeledKey {
            label: self.label.clone().unwrap_or_else(|| hex(&self.kid)),
            kid: self.kid,
            key: self.key,
            iv: None,
        }
    }
}

fn attr(elem: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let i = elem.find(&pat)?;
    let rest = &elem[i + pat.len()..];
    let j = rest.find('"')?;
    Some(rest[..j].to_string())
}

fn tag_text(elem: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let i = elem.find(&open)?;
    let rest = &elem[i + open.len()..];
    let j = rest.find(&close)?;
    Some(rest[..j].trim().to_string())
}

fn parse_kid(s: &str) -> Result<[u8; 16]> {
    let t = s.trim().trim_start_matches("urn:uuid:").replace('-', "");
    if t.len() == 32 {
        parse_hex16(&t)
    } else {
        // base64
        let b = b64_decode(s)?;
        anyhow::ensure!(b.len() == 16, "CPIX kid must be 16 bytes");
        Ok(b.try_into().unwrap())
    }
}

fn decode_key(s: &str) -> Result<[u8; 16]> {
    let t = s.trim().replace('-', "");
    if t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        return parse_hex16(&t);
    }
    let b = b64_decode(s)?;
    anyhow::ensure!(b.len() == 16, "CPIX key must be 16 bytes, got {}", b.len());
    Ok(b.try_into().unwrap())
}

fn decrypt_cpix_secret(b64: &str, pem: &str) -> Result<[u8; 16]> {
    use rsa::Oaep;
    use rsa::pkcs1::DecodeRsaPrivateKey;
    use rsa::pkcs8::DecodePrivateKey;
    use sha2::Sha256;
    let key = rsa::RsaPrivateKey::from_pkcs1_pem(pem)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs8_pem(pem))
        .context("parsing CPIX recipient RSA private key")?;
    let ct = b64_decode(b64)?;
    let pt = key.decrypt(Oaep::new::<Sha256>(), &ct).context("RSA-OAEP decrypt of CPIX secret")?;
    anyhow::ensure!(pt.len() == 16, "decrypted CPIX key is {} bytes", pt.len());
    Ok(pt.try_into().unwrap())
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    crate::widevine::b64_decode_pub(s)
}

fn hex(b: &[u8; 16]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_cpix() {
        let xml = r#"<CPIX xmlns="urn:dashif:org:cpix">
  <ContentKeyList>
    <ContentKey kid="00112233445566778899aabbccddeeff">
      <Data><Secret><PlainValue>AAECAwQFBgcICQoLDA0ODw==</PlainValue></Secret></Data>
    </ContentKey>
  </ContentKeyList>
  <UsageRuleList>
    <UsageRule kid="00112233445566778899aabbccddeeff" intendedTrackType="HD"/>
  </UsageRuleList>
</CPIX>"#;
        let keys = parse_cpix(xml, None).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].kid[0], 0x00);
        assert_eq!(keys[0].key[0], 0x00);
        assert_eq!(keys[0].key[1], 0x01);
        assert_eq!(keys[0].label.as_deref(), Some("HD"));
    }
}

//! HTTP(S) client helpers: plain HTTP via std, HTTPS via rustls.

use anyhow::{Context, Result, bail};
use rustls_pki_types::pem::PemObject;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// TLS / HTTP options for push, CPIX, and Widevine.
#[derive(Debug, Clone, Default)]
pub struct HttpOptions {
    pub user_agent: Option<String>,
    pub ca_file: Option<std::path::PathBuf>,
    pub client_cert_file: Option<std::path::PathBuf>,
    pub client_cert_key_file: Option<std::path::PathBuf>,
    pub client_cert_key_password: Option<String>,
    pub disable_peer_verification: bool,
    pub ignore_failures: bool,
}

/// GET `url`, return response body.
pub(crate) fn http_get(
    url: &str,
    opts: &HttpOptions,
    extra_headers: &[(&str, &str)],
) -> Result<Vec<u8>> {
    http_exchange("GET", url, None, None, opts, extra_headers)
}

/// POST `body` to `url`.
pub(crate) fn http_post(
    url: &str,
    body: &[u8],
    content_type: &str,
    opts: &HttpOptions,
    extra_headers: &[(&str, &str)],
) -> Result<Vec<u8>> {
    http_exchange("POST", url, Some(body), Some(content_type), opts, extra_headers)
}

/// PUT `body` to `url`.
pub(crate) fn http_put(url: &str, body: &[u8], opts: &HttpOptions) -> Result<()> {
    match http_exchange("PUT", url, Some(body), Some("application/octet-stream"), opts, &[]) {
        Ok(_) => Ok(()),
        Err(e) if opts.ignore_failures => {
            eprintln!("http: ignoring PUT failure for {url}: {e:#}");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn http_exchange(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
    opts: &HttpOptions,
    extra_headers: &[(&str, &str)],
) -> Result<Vec<u8>> {
    let https = url.starts_with("https://");
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .context("URL must be http:// or https://")?;
    let (hostport, path) =
        rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let host = hostport.split(':').next().unwrap_or(hostport);
    let default_port = if https { 443 } else { 80 };
    let addr_s = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:{default_port}")
    };
    let addr = addr_s
        .to_socket_addrs()
        .with_context(|| format!("resolving {addr_s}"))?
        .next()
        .context("no addresses")?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(15))
        .with_context(|| format!("connecting to {addr_s}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(60)))?;

    let ua = opts.user_agent.as_deref().unwrap_or("sheathe");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {ua}\r\nConnection: close\r\n"
    );
    if let Some(ct) = content_type {
        req.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");

    let raw = if https {
        let mut stream = tls_wrap(tcp, host, opts)?;
        stream.write_all(req.as_bytes())?;
        if let Some(b) = body {
            stream.write_all(b)?;
        }
        stream.flush()?;
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).ok();
        resp
    } else {
        let mut stream = tcp;
        stream.write_all(req.as_bytes())?;
        if let Some(b) = body {
            stream.write_all(b)?;
        }
        stream.flush()?;
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).ok();
        resp
    };
    split_http_body(&raw)
}

fn split_http_body(raw: &[u8]) -> Result<Vec<u8>> {
    let text = String::from_utf8_lossy(raw);
    let status = text.lines().next().unwrap_or("");
    let ok = status.contains(" 200")
        || status.contains(" 201")
        || status.contains(" 204")
        || status.contains(" 100");
    if !ok && !status.starts_with("HTTP/") {
        // maybe just the body
        return Ok(raw.to_vec());
    }
    if !ok {
        bail!("HTTP error: {status}");
    }
    if let Some(idx) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Ok(raw[idx + 4..].to_vec())
    } else {
        Ok(raw.to_vec())
    }
}

fn tls_wrap(
    tcp: TcpStream,
    host: &str,
    opts: &HttpOptions,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>> {
    let mut root = rustls::RootCertStore::empty();
    root.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(ca) = &opts.ca_file {
        let pem = fs_read(ca)?;
        for cert in rustls_pki_types::CertificateDer::pem_slice_iter(&pem).flatten() {
            root.add(cert).ok();
        }
    }
    if opts.disable_peer_verification {
        // Still use roots; rustls has no official skip. Document that
        // disable_peer_verification currently only skips adding extra checks
        // beyond the platform roots — we refuse to silently disable TLS.
        eprintln!(
            "sheathe: --disable-peer-verification is ignored for rustls (roots still enforced)"
        );
    }
    let builder = rustls::ClientConfig::builder().with_root_certificates(root);
    let config =
        if let (Some(cert), Some(key)) = (&opts.client_cert_file, &opts.client_cert_key_file) {
            let certs = load_certs(cert)?;
            let key = load_key(key, opts.client_cert_key_password.as_deref())?;
            builder.with_client_auth_cert(certs, key).context("client cert")?
        } else {
            builder.with_no_client_auth()
        };
    let server = rustls::pki_types::ServerName::try_from(host.to_string()).context("SNI")?;
    let conn = rustls::ClientConnection::new(Arc::new(config), server).context("TLS client")?;
    Ok(rustls::StreamOwned::new(conn, tcp))
}

fn load_certs(path: &std::path::Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    rustls_pki_types::CertificateDer::pem_file_iter(path)
        .with_context(|| format!("reading certs {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .context("parsing certs")
}

fn load_key(
    path: &std::path::Path,
    _password: Option<&str>,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    rustls_pki_types::PrivateKeyDer::from_pem_file(path)
        .with_context(|| format!("reading key {}", path.display()))
}

fn fs_read(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// rustls server config from PEM cert + key (origin TLS).
pub(crate) fn server_config(
    cert: &std::path::Path,
    key: &std::path::Path,
) -> Result<Arc<rustls::ServerConfig>> {
    let certs = load_certs(cert)?;
    let key = load_key(key, None)?;
    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("origin TLS cert")?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

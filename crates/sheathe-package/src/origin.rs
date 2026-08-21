//! JIT / origin mode — package on HTTP request (Phase 5).
//!
//! A minimal HTTP/1.1 origin that serves:
//! - `GET /health` → `ok`
//! - `GET /package?input=<path>&…` → runs [`crate::package`] into a temp dir and
//!   returns `master.m3u8` or `manifest.mpd` based on `Accept` / `format=`
//! - `GET /out/<rel>` → serves a previously packaged object from the origin cache
//!
//! Optional HTTPS (`--tls-cert` / `--tls-key`) is rustls; optional Basic auth
//! is `--auth user:password`. Not a production CDN.

use crate::{PackageOptions, PresentationMode, package};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

/// Configuration for [`serve`].
#[derive(Debug, Clone)]
pub struct OriginConfig {
    /// Bind address, e.g. `127.0.0.1:8787`.
    pub bind: String,
    /// Directory holding source media (path sandbox for `input=`).
    pub media_root: PathBuf,
    /// Cache directory for packaged outputs.
    pub cache_dir: PathBuf,
    /// Default segment duration seconds.
    pub segment_duration: f64,
    /// Optional TLS certificate PEM (enables HTTPS).
    pub tls_cert: Option<PathBuf>,
    /// Optional TLS private key PEM.
    pub tls_key: Option<PathBuf>,
    /// Optional `user:password` for HTTP Basic auth.
    pub basic_auth: Option<String>,
}

impl Default for OriginConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            media_root: PathBuf::from("."),
            cache_dir: PathBuf::from("/tmp/sheathe-origin"),
            segment_duration: 6.0,
            tls_cert: None,
            tls_key: None,
            basic_auth: None,
        }
    }
}

/// Run the origin until the process is killed. Spawns one thread per connection.
pub fn serve(cfg: OriginConfig) -> Result<()> {
    fs::create_dir_all(&cfg.cache_dir)?;
    let listener = TcpListener::bind(&cfg.bind).with_context(|| format!("bind {}", cfg.bind))?;
    serve_listener(listener, cfg)
}

fn serve_listener(listener: TcpListener, cfg: OriginConfig) -> Result<()> {
    let tls = match (&cfg.tls_cert, &cfg.tls_key) {
        (None, None) => None,
        (Some(cert), Some(key)) => Some(crate::tls::server_config(cert, key)?),
        _ => bail!("origin TLS requires both --tls-cert and --tls-key"),
    };
    let scheme = if tls.is_some() { "https" } else { "http" };
    let addr = listener.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| cfg.bind.clone());
    eprintln!("sheathe origin listening on {scheme}://{addr}/");
    let cfg = Arc::new(cfg);
    let lock = Arc::new(Mutex::new(()));
    for conn in listener.incoming() {
        let stream = conn.context("accept")?;
        let cfg = Arc::clone(&cfg);
        let lock = Arc::clone(&lock);
        let tls = tls.clone();
        thread::spawn(move || {
            if let Err(e) = dispatch_client(stream, tls, &cfg, &lock) {
                eprintln!("origin: {e:#}");
            }
        });
    }
    Ok(())
}

fn dispatch_client(
    stream: TcpStream,
    tls: Option<Arc<rustls::ServerConfig>>,
    cfg: &OriginConfig,
    lock: &Mutex<()>,
) -> Result<()> {
    match tls {
        Some(tls) => {
            let conn = rustls::ServerConnection::new(tls).context("TLS server")?;
            let mut tls_stream = rustls::StreamOwned::new(conn, stream);
            handle_client(&mut tls_stream, cfg, lock)
        }
        None => {
            let mut stream = stream;
            handle_client(&mut stream, cfg, lock)
        }
    }
}

fn handle_client<S: Read + Write>(
    stream: &mut S,
    cfg: &OriginConfig,
    lock: &Mutex<()>,
) -> Result<()> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    if let Some(auth) = &cfg.basic_auth
        && !authorized(&req, auth)
    {
        return respond(
            stream,
            401,
            "text/plain",
            b"unauthorized\n",
            "WWW-Authenticate: Basic realm=\"sheathe\"\r\n",
        );
    }
    let line = req.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return respond(stream, 405, "text/plain", b"method not allowed", "");
    }

    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q = parse_query(query);

    match path {
        "/health" | "/healthz" => respond(stream, 200, "text/plain", b"ok\n", ""),
        "/package" => {
            let input = q.get("input").map(String::as_str).context("missing input=")?;
            let media = resolve_media(&cfg.media_root, input)?;
            let format = q.get("format").map(String::as_str).unwrap_or("hls");
            let key = cache_key(&media, format, cfg.segment_duration);
            let out_dir = cfg.cache_dir.join(&key);
            {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                if !out_dir.join("master.m3u8").exists() && !out_dir.join("manifest.mpd").exists() {
                    let opts = PackageOptions {
                        out_dir: out_dir.clone(),
                        segment_duration: cfg.segment_duration,
                        dash: format == "dash" || format == "both",
                        hls: format == "hls" || format == "both",
                        presentation: PresentationMode::Vod,
                        ..PackageOptions::default()
                    };
                    package(&[media], &opts)?;
                }
            }
            let body_path = if format == "dash" {
                out_dir.join("manifest.mpd")
            } else {
                out_dir.join("master.m3u8")
            };
            let body =
                fs::read(&body_path).with_context(|| format!("read {}", body_path.display()))?;
            let ctype = if format == "dash" {
                "application/dash+xml"
            } else {
                "application/vnd.apple.mpegurl"
            };
            respond(stream, 200, ctype, &body, "")
        }
        p if p.starts_with("/out/") => {
            let rel = &p["/out/".len()..];
            let path = cfg.cache_dir.join(rel);
            // Prevent path escape.
            let canon_cache =
                cfg.cache_dir.canonicalize().unwrap_or_else(|_| cfg.cache_dir.clone());
            let canon = path.canonicalize().with_context(|| format!("missing {rel}"))?;
            if !canon.starts_with(&canon_cache) {
                bail!("path escape");
            }
            let body = fs::read(&canon)?;
            respond(stream, 200, guess_ctype(rel), &body, "")
        }
        _ => respond(
            stream,
            404,
            "text/plain",
            b"not found\nGET /health | /package?input=FILE&format=hls|dash | /out/<cache-rel>\n",
            "",
        ),
    }
}

fn resolve_media(root: &Path, input: &str) -> Result<PathBuf> {
    let p = Path::new(input);
    let full = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    let canon = full.canonicalize().with_context(|| format!("input not found: {input}"))?;
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !canon.starts_with(&root_canon) && !Path::new(input).is_absolute() {
        bail!("input escapes media_root");
    }
    Ok(canon)
}

fn cache_key(media: &Path, format: &str, seg: f64) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    media.hash(&mut h);
    format.hash(&mut h);
    seg.to_bits().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for pair in q.split('&').filter(|s| !s.is_empty()) {
        if let Some((k, v)) = pair.split_once('=') {
            m.insert(url_decode(k), url_decode(v));
        }
    }
    m
}

fn url_decode(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v as char);
                    i += 3;
                } else {
                    out.push('%');
                    i += 1;
                }
            }
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    out
}

fn guess_ctype(path: &str) -> &'static str {
    if path.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if path.ends_with(".mpd") {
        "application/dash+xml"
    } else if path.ends_with(".mp4") || path.ends_with(".m4s") {
        "video/mp4"
    } else if path.ends_with(".ts") {
        "video/mp2t"
    } else {
        "application/octet-stream"
    }
}

fn authorized(req: &str, user_pass: &str) -> bool {
    let want = b64_basic(user_pass.as_bytes());
    req.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else { return false };
        if !name.eq_ignore_ascii_case("authorization") {
            return false;
        }
        let value = value.trim();
        value
            .strip_prefix("Basic ")
            .or_else(|| value.strip_prefix("basic "))
            .is_some_and(|b64| b64.trim() == want)
    })
}

fn b64_basic(raw: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::new();
    for chunk in raw.chunks(3) {
        let x = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(x[0]) << 16) | (u32::from(x[1]) << 8) | u32::from(x[2]);
        b64.push(A[(n >> 18 & 0x3f) as usize] as char);
        b64.push(A[(n >> 12 & 0x3f) as usize] as char);
        b64.push(if chunk.len() > 1 { A[(n >> 6 & 0x3f) as usize] as char } else { '=' });
        b64.push(if chunk.len() > 2 { A[(n & 0x3f) as usize] as char } else { '=' });
    }
    b64
}

fn respond<S: Write>(
    stream: &mut S,
    status: u16,
    ctype: &str,
    body: &[u8],
    extra_headers: &str,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n{extra_headers}\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;

    fn spawn_origin(cfg: OriginConfig) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let _ = serve_listener(listener, cfg);
        });
        // Accept loop is ready as soon as bind succeeded.
        addr
    }

    fn http_exchange(addr: SocketAddr, req: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).ok();
        s.set_write_timeout(Some(Duration::from_secs(2))).ok();
        s.write_all(req.as_bytes()).unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).ok();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[test]
    fn health_over_plain_http() {
        let cfg = OriginConfig { bind: "127.0.0.1:0".into(), ..OriginConfig::default() };
        let addr = spawn_origin(cfg);
        let resp = http_exchange(
            addr,
            "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(resp.contains("HTTP/1.1 200"), "{resp}");
        assert!(resp.contains("ok"), "{resp}");
    }

    #[test]
    fn basic_auth_rejects_and_accepts() {
        let cfg = OriginConfig {
            bind: "127.0.0.1:0".into(),
            basic_auth: Some("alice:secret".into()),
            ..OriginConfig::default()
        };
        let addr = spawn_origin(cfg);
        let denied = http_exchange(
            addr,
            "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(denied.contains("401"), "{denied}");
        assert!(denied.contains("WWW-Authenticate: Basic"), "{denied}");

        let cred = b64_basic(b"alice:secret");
        let ok = http_exchange(
            addr,
            &format!(
                "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Basic {cred}\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(ok.contains("HTTP/1.1 200"), "{ok}");
    }

    #[test]
    fn tls_requires_both_cert_and_key() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = OriginConfig {
            tls_cert: Some(PathBuf::from("/tmp/sheathe-missing.pem")),
            ..OriginConfig::default()
        };
        let err = serve_listener(listener, cfg).unwrap_err();
        assert!(err.to_string().contains("both"), "{err:#}");
    }

    #[test]
    fn health_over_https() {
        let ck = rcgen::generate_simple_self_signed(["localhost".into()]).unwrap();
        let dir = std::env::temp_dir().join(format!("sheathe-origin-tls-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let cert_path = dir.join("cert.pem");
        let key_path = dir.join("key.pem");
        fs::write(&cert_path, ck.cert.pem()).unwrap();
        fs::write(&key_path, ck.key_pair.serialize_pem()).unwrap();

        let cfg = OriginConfig {
            bind: "127.0.0.1:0".into(),
            tls_cert: Some(cert_path.clone()),
            tls_key: Some(key_path),
            ..OriginConfig::default()
        };
        let addr = spawn_origin(cfg);

        let mut roots = rustls::RootCertStore::empty();
        let pem = fs::read(&cert_path).unwrap();
        for cert in rustls_pemfile::certs(&mut pem.as_slice()).flatten() {
            roots.add(cert).unwrap();
        }
        let config =
            rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let tcp = TcpStream::connect(addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        tls.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        tls.read_to_end(&mut buf).ok();
        let resp = String::from_utf8_lossy(&buf);
        assert!(resp.contains("HTTP/1.1 200"), "{resp}");
        assert!(resp.contains("ok"), "{resp}");
    }
}

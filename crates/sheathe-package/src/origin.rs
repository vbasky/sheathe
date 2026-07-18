//! JIT / origin mode — package on HTTP request (Phase 5).
//!
//! A minimal pure-`std` HTTP/1.1 origin that serves:
//! - `GET /health` → `ok`
//! - `GET /package?input=<path>&…` → runs [`crate::package`] into a temp dir and
//!   returns `master.m3u8` or `manifest.mpd` based on `Accept` / `format=`
//! - `GET /out/<rel>` → serves a previously packaged object from the origin cache
//!
//! Not a production CDN; it demonstrates JIT packaging without external deps.

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
}

impl Default for OriginConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            media_root: PathBuf::from("."),
            cache_dir: PathBuf::from("/tmp/sheathe-origin"),
            segment_duration: 6.0,
        }
    }
}

/// Run the origin until the process is killed. Spawns one thread per connection.
pub fn serve(cfg: OriginConfig) -> Result<()> {
    fs::create_dir_all(&cfg.cache_dir)?;
    let listener = TcpListener::bind(&cfg.bind).with_context(|| format!("bind {}", cfg.bind))?;
    eprintln!("sheathe origin listening on http://{}/", cfg.bind);
    let cfg = Arc::new(cfg);
    let lock = Arc::new(Mutex::new(()));
    for conn in listener.incoming() {
        let stream = conn.context("accept")?;
        let cfg = Arc::clone(&cfg);
        let lock = Arc::clone(&lock);
        thread::spawn(move || {
            if let Err(e) = handle_client(stream, &cfg, &lock) {
                eprintln!("origin: {e:#}");
            }
        });
    }
    Ok(())
}

fn handle_client(mut stream: TcpStream, cfg: &OriginConfig, lock: &Mutex<()>) -> Result<()> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let line = req.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return respond(&mut stream, 405, "text/plain", b"method not allowed");
    }

    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q = parse_query(query);

    match path {
        "/health" | "/healthz" => respond(&mut stream, 200, "text/plain", b"ok\n"),
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
            respond(&mut stream, 200, ctype, &body)
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
            respond(&mut stream, 200, guess_ctype(rel), &body)
        }
        _ => respond(
            &mut stream,
            404,
            "text/plain",
            b"not found\nGET /health | /package?input=FILE&format=hls|dash | /out/<cache-rel>\n",
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

fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

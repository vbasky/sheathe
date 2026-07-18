//! Output sinks and input sources for packaging (Phase 5 IO backends).
//!
//! - [`FileSink`] — write relative paths under an output directory (default).
//! - [`HttpPushSink`] — HTTP/1.1 `PUT` each object to a base URL (pure `std`).
//! - [`read_input`] — load bytes from a file path or `udp://host:port` (single
//!   datagram / short capture window for live ingest demos).

use anyhow::{Context, Result, bail};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where packaged objects (segments, manifests) are written.
pub trait ObjectSink: Send {
    /// Write `data` at a path relative to the sink root.
    fn put(&mut self, relative: &str, data: &[u8]) -> Result<()>;
    /// Human-readable root (directory or base URL) for logging.
    fn root(&self) -> String;
}

/// Local filesystem sink.
#[derive(Debug, Clone)]
pub struct FileSink {
    /// Output directory (created on first write if missing).
    pub dir: PathBuf,
}

impl FileSink {
    /// Create a sink rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

impl ObjectSink for FileSink {
    fn put(&mut self, relative: &str, data: &[u8]) -> Result<()> {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, data).with_context(|| format!("writing {}", path.display()))
    }

    fn root(&self) -> String {
        self.dir.display().to_string()
    }
}

/// HTTP/1.1 PUT sink — pushes each object to `{base_url}/{relative}`.
///
/// Pure std: opens a TCP connection per object and writes a minimal request.
/// Intended for origin CDN ingest / PUT-capable static hosts, not multipart.
#[derive(Debug, Clone)]
pub struct HttpPushSink {
    /// Base URL without trailing slash, e.g. `http://127.0.0.1:8080/live`.
    pub base_url: String,
    /// Optional `Authorization` header value.
    pub authorization: Option<String>,
}

impl HttpPushSink {
    /// Create a push sink for `base_url`.
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base = base_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self { base_url: base, authorization: None }
    }
}

impl ObjectSink for HttpPushSink {
    fn put(&mut self, relative: &str, data: &[u8]) -> Result<()> {
        let url = format!("{}/{}", self.base_url, relative.trim_start_matches('/'));
        http_put(&url, data, self.authorization.as_deref())
            .with_context(|| format!("HTTP PUT {url}"))
    }

    fn root(&self) -> String {
        self.base_url.clone()
    }
}

/// Parse `http://host:port/path` and PUT `body`.
fn http_put(url: &str, body: &[u8], authorization: Option<&str>) -> Result<()> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .context("only http:// and https:// push URLs are supported")?;
    if url.starts_with("https://") {
        bail!(
            "HTTPS push requires TLS; use http:// for the pure-std push sink or terminate TLS externally"
        );
    }
    let (hostport, path) =
        rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let host = hostport.split(':').next().unwrap_or(hostport);
    let addr = hostport
        .to_socket_addrs()
        .with_context(|| format!("resolving {hostport}"))?
        .next()
        .context("no addresses for push host")?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .with_context(|| format!("connecting to {hostport}"))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;

    let mut req = format!(
        "PUT {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(auth) = authorization {
        req.push_str(&format!("Authorization: {auth}\r\n"));
    }
    req.push_str("Content-Type: application/octet-stream\r\n\r\n");
    stream.write_all(req.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).ok();
    let text = String::from_utf8_lossy(&resp);
    let status = text.lines().next().unwrap_or("");
    if !(status.contains(" 200 ")
        || status.contains(" 201 ")
        || status.contains(" 204 ")
        || status.contains(" 100 "))
    {
        // Some servers speak HTTP/1.0 without spaces the same way — accept 2xx.
        let ok = text.contains(" 2") && status.starts_with("HTTP/");
        if !ok {
            bail!("push rejected: {status}");
        }
    }
    Ok(())
}

/// Read an input path: plain filesystem, or `udp://bind_host:port` for a short
/// live capture (collects datagrams for `duration` or until ~4 MiB).
pub fn read_input(spec: &str) -> Result<(String, Vec<u8>)> {
    if let Some(addr) = spec.strip_prefix("udp://") {
        let data = udp_ingest(addr, Duration::from_secs(3), 4 * 1024 * 1024)?;
        return Ok((spec.to_string(), data));
    }
    let path = Path::new(spec);
    let data = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok((spec.to_string(), data))
}

/// Bind `addr` (e.g. `0.0.0.0:5000`) and collect UDP datagrams until `max_bytes`
/// or `timeout` elapses with no further data after the first packet.
fn udp_ingest(addr: &str, timeout: Duration, max_bytes: usize) -> Result<Vec<u8>> {
    let sock = UdpSocket::bind(addr).with_context(|| format!("binding UDP {addr}"))?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    let mut buf = vec![0u8; 65535];
    let mut out = Vec::new();
    let start = std::time::Instant::now();
    let mut got_any = false;
    while out.len() < max_bytes && start.elapsed() < timeout {
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                out.extend_from_slice(&buf[..n]);
                got_any = true;
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if got_any {
                    // Quiet period after first data — stop.
                    break;
                }
            }
            Err(e) => return Err(e).context("UDP recv"),
        }
    }
    anyhow::ensure!(got_any, "UDP ingest on {addr}: no datagrams received within {timeout:?}");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn file_sink_writes_relative() {
        let dir = std::env::temp_dir().join(format!("sheathe-io-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut sink = FileSink::new(&dir);
        sink.put("a/b.txt", b"hello").unwrap();
        assert_eq!(fs::read(dir.join("a/b.txt")).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn http_put_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            // Read full request (headers + 9-byte body).
            loop {
                match sock.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n")
                            && buf.windows(9).any(|w| w == b"hello-seg")
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let req = String::from_utf8_lossy(&buf);
            assert!(req.contains("PUT /live/seg.m4s "), "req={req}");
            assert!(req.contains("hello-seg"), "req={req}");
            sock.write_all(
                b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        });
        let mut sink = HttpPushSink::new(format!("http://127.0.0.1:{port}/live"));
        sink.put("seg.m4s", b"hello-seg").unwrap();
        handle.join().unwrap();
    }
}

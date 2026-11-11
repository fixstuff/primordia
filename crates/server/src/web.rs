//! The viewer's transport: a minimal HTTP/1.1 responder for the page and a
//! read-only spectator WebSocket (RFC 6455) at /ws. Hand-written because no
//! HTTP or WebSocket crate is in the local cache (design doc D-4).
//!
//! Scope, deliberately small: GET only, one request per connection, no
//! keep-alive, no compression, no fragmentation on send. The client is
//! expected to send nothing but a close frame; anything it sends is read and
//! dropped. Loopback only, like the API (see `crate::bind`).

use crate::{snapshot::Snapshot, Msg};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha1::{Digest, Sha1};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

const INDEX_HTML: &str = include_str!("../web/index.html");
const VIEWER_JS: &str = include_str!("../web/viewer.js");
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_HEAD_BYTES: usize = 8 * 1024;
const MAX_CLIENT_FRAME: u64 = 64 * 1024;
/// How often a spectator asks for a new view; it is sent only if changed.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) fn serve(listener: TcpListener, tx: mpsc::Sender<Msg>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = handle(stream, tx);
        });
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn read_head(r: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut total = 0;
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let n = r.by_ref().take((MAX_HEAD_BYTES - total) as u64 + 1).read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        total += n;
        if total > MAX_HEAD_BYTES {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    let Some(first) = lines.first() else { return Ok(None) };
    let mut parts = first.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else { return Ok(None) };
    let headers = lines[1..]
        .iter()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Ok(Some(Request { method: method.to_string(), path: path.to_string(), headers }))
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)?;
    s.flush()
}

fn handle(stream: TcpStream, tx: mpsc::Sender<Msg>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut w = stream.try_clone()?;
    let mut r = BufReader::new(stream);
    let Some(req) = read_head(&mut r)? else { return Ok(()) };
    if req.method != "GET" {
        return respond(&mut w, "405 Method Not Allowed", "text/plain", b"GET only\n");
    }
    let path = req.path.split('?').next().unwrap_or("");
    match path {
        "/" | "/index.html" => respond(&mut w, "200 OK", "text/html; charset=utf-8", INDEX_HTML.as_bytes()),
        "/viewer.js" => respond(&mut w, "200 OK", "text/javascript; charset=utf-8", VIEWER_JS.as_bytes()),
        "/inspect" => {
            let id = req.path.split_once("id=").and_then(|(_, v)| v.split('&').next()?.parse::<u64>().ok());
            let Some(id) = id else {
                return respond(&mut w, "400 Bad Request", "text/plain", b"inspect?id=N\n");
            };
            let (rtx, rrx) = mpsc::channel();
            if tx.send(Msg::Inspect { id, reply: rtx }).is_err() {
                return respond(&mut w, "503 Service Unavailable", "text/plain", b"stopped\n");
            }
            let body = rrx.recv().ok().flatten().unwrap_or_else(|| "{\"gone\":true}".to_string());
            respond(&mut w, "200 OK", "application/json", body.as_bytes())
        }
        "/ws" => {
            let upgrade = req.header("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
            let Some(key) = req.header("sec-websocket-key").filter(|_| upgrade) else {
                return respond(&mut w, "400 Bad Request", "text/plain", b"WebSocket upgrade required\n");
            };
            write!(
                w,
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {}\r\n\r\n",
                accept_key(key)
            )?;
            w.flush()?;
            spectate(r, w, tx)
        }
        _ => respond(&mut w, "404 Not Found", "text/plain", b"not found\n"),
    }
}

/// RFC 6455 §4.2.2: base64(SHA-1(key + GUID)).
pub fn accept_key(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.as_bytes());
    h.update(WS_GUID.as_bytes());
    B64.encode(h.finalize())
}

/// One unmasked, unfragmented server frame (RFC 6455 §5.2).
pub fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 10);
    f.push(0x80 | opcode);
    let n = payload.len();
    if n < 126 {
        f.push(n as u8);
    } else if n <= 0xFFFF {
        f.push(126);
        f.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        f.push(127);
        f.extend_from_slice(&(n as u64).to_be_bytes());
    }
    f.extend_from_slice(payload);
    f
}

/// Read and discard client frames until close or EOF, then raise `closed`.
fn drain_client(mut r: BufReader<TcpStream>, closed: Arc<AtomicBool>) {
    let _ = r.get_ref().set_read_timeout(None);
    let res: io::Result<()> = (|| loop {
        let mut h = [0u8; 2];
        r.read_exact(&mut h)?;
        let opcode = h[0] & 0x0F;
        let masked = h[1] & 0x80 != 0;
        let mut len = (h[1] & 0x7F) as u64;
        if len == 126 {
            let mut b = [0u8; 2];
            r.read_exact(&mut b)?;
            len = u16::from_be_bytes(b) as u64;
        } else if len == 127 {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            len = u64::from_be_bytes(b);
        }
        if len > MAX_CLIENT_FRAME {
            return Ok(());
        }
        let mut skip = len + if masked { 4 } else { 0 };
        let mut buf = [0u8; 1024];
        while skip > 0 {
            let k = skip.min(buf.len() as u64) as usize;
            r.read_exact(&mut buf[..k])?;
            skip -= k as u64;
        }
        if opcode == 0x8 {
            return Ok(());
        }
    })();
    let _ = res;
    closed.store(true, Ordering::Relaxed);
}

fn spectate(r: BufReader<TcpStream>, mut w: TcpStream, tx: mpsc::Sender<Msg>) -> io::Result<()> {
    let closed = Arc::new(AtomicBool::new(false));
    {
        let closed = closed.clone();
        thread::spawn(move || drain_client(r, closed));
    }
    let (rtx, rrx) = mpsc::channel::<Option<Snapshot>>();
    let mut version = None;
    let mut history_after = None;
    while !closed.load(Ordering::Relaxed) {
        if tx.send(Msg::Snapshot { since_version: version, history_after, reply: rtx.clone() }).is_err() {
            break;
        }
        let Ok(snap) = rrx.recv() else { break };
        if let Some(s) = snap {
            w.write_all(&frame(0x1, s.meta.as_bytes()))?;
            w.write_all(&frame(0x2, &s.frame))?;
            w.flush()?;
            version = Some(s.version);
            history_after = s.history_tick;
        }
        thread::sleep(FRAME_INTERVAL);
    }
    let _ = w.write_all(&frame(0x8, &[]));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_rfc6455_example() {
        // RFC 6455 §1.3.
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frame_length_encodings() {
        assert_eq!(&frame(0x2, &[7; 125])[..2], &[0x82, 125]);
        assert_eq!(&frame(0x2, &[7; 126])[..4], &[0x82, 126, 0, 126]);
        assert_eq!(&frame(0x2, &[7; 65535])[..4], &[0x82, 126, 0xFF, 0xFF]);
        let big = frame(0x2, &vec![7; 65536]);
        assert_eq!(&big[..10], &[0x82, 127, 0, 0, 0, 0, 0, 1, 0, 0]);
        assert_eq!(big.len(), 10 + 65536);
    }
}

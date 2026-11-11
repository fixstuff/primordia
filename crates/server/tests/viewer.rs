//! The viewer's transport, end to end over real TCP: the page and script are
//! served, the WebSocket handshake completes with the right accept key, and
//! a spectator receives a meta message and a binary frame that follow a
//! player's step.

use primordia_server::{bind, serve_with_viewer, web, Game};
use primordia_sim::scenario;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn start() -> (String, String) {
    let api = bind("127.0.0.1:0").unwrap();
    let http = bind("127.0.0.1:0").unwrap();
    let (a, h) = (api.local_addr().unwrap().to_string(), http.local_addr().unwrap().to_string());
    let game = Game::new(scenario::pool(scenario::base_config(5, 128, 128), 8, 4));
    thread::spawn(move || serve_with_viewer(api, Some(http), game));
    (a, h)
}

fn get(addr: &str, path: &str) -> (String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (head.to_string(), body.to_string())
}

/// Read one server frame: (opcode, payload). Server frames are unmasked.
fn read_frame(r: &mut impl Read) -> (u8, Vec<u8>) {
    let mut h = [0u8; 2];
    r.read_exact(&mut h).unwrap();
    assert_eq!(h[1] & 0x80, 0, "server frames must not be masked");
    let mut len = (h[1] & 0x7F) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        r.read_exact(&mut b).unwrap();
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        r.read_exact(&mut b).unwrap();
        len = u64::from_be_bytes(b);
    }
    let mut p = vec![0u8; len as usize];
    r.read_exact(&mut p).unwrap();
    (h[0] & 0x0F, p)
}

fn api_call(c: &mut (TcpStream, BufReader<TcpStream>), line: &str) -> serde_json::Value {
    writeln!(c.0, "{line}").unwrap();
    let mut s = String::new();
    c.1.read_line(&mut s).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["ok"], true, "{line} -> {v}");
    v
}

#[test]
fn page_script_and_404() {
    let (_, http) = start();
    let (head, body) = get(&http, "/");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(body.contains("<script src=\"viewer.js\">"));
    let (head, body) = get(&http, "/viewer.js");
    assert!(head.contains("text/javascript"), "{head}");
    assert!(body.contains("new WebSocket"));
    let (head, _) = get(&http, "/nope");
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
}

#[test]
fn spectator_sees_meta_and_frame_and_follows_a_step() {
    let (api, http) = start();
    let mut ws = TcpStream::connect(&http).unwrap();
    ws.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        ws,
        "GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .unwrap();
    let mut r = BufReader::new(ws.try_clone().unwrap());
    let mut head = String::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        if l == "\r\n" {
            break;
        }
        head.push_str(&l);
    }
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.contains(&format!("Sec-WebSocket-Accept: {}", web::accept_key("dGhlIHNhbXBsZSBub25jZQ=="))));

    // First: the whole state at tick 0.
    let (op, meta) = read_frame(&mut r);
    assert_eq!(op, 0x1);
    let meta: serde_json::Value = serde_json::from_slice(&meta).unwrap();
    assert_eq!(meta["tick"], 0);
    assert_eq!(meta["history_reset"], true);
    let (op, frame) = read_frame(&mut r);
    assert_eq!(op, 0x2);
    assert_eq!(&frame[0..4], b"LAPI");

    // A player releases and steps; the spectator sees it.
    let s = TcpStream::connect(&api).unwrap();
    let mut c = (s.try_clone().unwrap(), BufReader::new(s));
    api_call(&mut c, r#"{"op":"hello","name":"p"}"#);
    // One request per line: the genome must go out as single-line JSON.
    let genome: serde_json::Value = serde_json::from_str(primordia_sim::scenario::FOUNDER).unwrap();
    let sp = api_call(&mut c, &format!(r#"{{"op":"submit_genome","genome":{genome}}}"#))["species"].clone();
    api_call(&mut c, &format!(r#"{{"op":"release","species":{sp},"count":12}}"#));
    api_call(&mut c, r#"{"op":"step","ticks":120}"#);

    let mut saw = None;
    for _ in 0..20 {
        let (op, p) = read_frame(&mut r);
        if op == 0x1 {
            let m: serde_json::Value = serde_json::from_slice(&p).unwrap();
            if m["tick"] == 120 {
                saw = Some(m);
                break;
            }
        }
    }
    let m = saw.expect("a meta message at tick 120");
    assert_eq!(m["history_reset"], false);
    assert_eq!(m["lineages"][0]["owner"], "p");
    assert_eq!(m["lineages"][0]["species_name"], "Driftwhip");
    let (op, frame) = read_frame(&mut r);
    assert_eq!(op, 0x2);
    assert_eq!(u64::from_le_bytes(frame[8..16].try_into().unwrap()), 120);
}

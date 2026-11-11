//! The MCP server as a client sees it: the real binary over stdio, talking
//! to a real lifeapi-server on an ephemeral loopback port.

use lifeapi_server::{bind, serve, Game};
use lifeapi_sim::scenario;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::thread;

struct Mcp {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Mcp {
    fn start(addr: &str) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lifeapi-mcp"))
            .args(["--addr", addr, "--name", "test-agent"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Mcp { child, out, next: 1 }
    }
    fn send(&mut self, v: Value) {
        writeln!(self.child.stdin.as_mut().unwrap(), "{v}").unwrap();
    }
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id);
        v
    }
    /// (text, is_error) of a tools/call.
    fn tool(&mut self, name: &str, args: Value) -> (Value, bool) {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        let text = r["result"]["content"][0]["text"].as_str().unwrap().to_string();
        let err = r["result"]["isError"].as_bool().unwrap();
        (serde_json::from_str(&text).unwrap_or(Value::String(text)), err)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn server() -> String {
    let l = bind("127.0.0.1:0").unwrap();
    let a = l.local_addr().unwrap().to_string();
    let game = Game::new(scenario::pool(scenario::base_config(9, 64, 64), 4, 2));
    thread::spawn(move || serve(l, game));
    a
}

#[test]
fn handshake_list_and_a_short_game() {
    let addr = server();
    let mut m = Mcp::start(&addr);
    let r = m.rpc("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}));
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert!(r["result"]["instructions"].as_str().unwrap().contains("step"));
    m.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})); // no reply expected

    let r = m.rpc("tools/list", json!({}));
    let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["catalog", "submit_genome", "release", "observe", "step", "report", "subscribe", "unsubscribe"]);
    for t in r["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
    }

    let (cat, err) = m.tool("catalog", json!({}));
    assert!(!err);
    assert_eq!(cat["genome"]["schema"], "lifeapi.genome/1");

    // A broken genome is a tool error naming the rule, not a protocol error.
    let mut bad: Value = serde_json::from_str(scenario::FOUNDER).unwrap();
    bad["body"][0]["radius"] = json!(99);
    let (text, err) = m.tool("submit_genome", json!({"genome": bad}));
    assert!(err);
    assert!(text.as_str().unwrap().contains("radius"), "{text}");

    let good: Value = serde_json::from_str(scenario::FOUNDER).unwrap();
    let (r, err) = m.tool("submit_genome", json!({"genome": good}));
    assert!(!err, "{r}");
    let (r, err) = m.tool("release", json!({"species": r["species"], "count": 30}));
    assert!(!err, "{r}");
    let lineage = r["lineage"].clone();
    let (_, err) = m.tool("subscribe", json!({"event": "tick", "value": 300}));
    assert!(!err);
    let (r, err) = m.tool("step", json!({"ticks": 5000}));
    assert!(!err);
    assert_eq!(r["tick"], 300, "subscription stopped the step: {r}");
    let (r, err) = m.tool("report", json!({}));
    assert!(!err);
    assert_eq!(r["lineages"][0]["lineage"], lineage);
    assert!(r["notes"].as_array().unwrap().len() >= 2, "{r}");

    let (t, err) = m.tool("fly", json!({}));
    assert!(err && t.as_str().unwrap().contains("unknown tool"));
    let r = m.rpc("bogus/method", json!({}));
    assert_eq!(r["error"]["code"], -32601);
}

/// The exact malformed calls a 4B model made in the first agent run
/// (fields beside `genome`, JSON as strings) are repaired by the adapter;
/// a genuinely bad genome still fails, now with a correct example.
#[test]
fn formatting_slips_are_repaired_but_rules_still_hold() {
    let addr = server();
    let mut m = Mcp::start(&addr);
    m.rpc("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}}));
    // Seen in the run: everything flattened, body/brain/mutation as strings.
    let (r, err) = m.tool("submit_genome", json!({
        "schema": "lifeapi.genome/1", "name": "Round1_BasicSwimmer",
        "body": "[{\"part\": \"membrane\", \"radius\": 5}, {\"part\": \"flagellum\", \"power\": 0.8}, {\"part\": \"mouth\"}]",
        "brain": "{\"hidden\": 4}", "mutation": "{\"rate\": 0.1, \"size\": 0.2}"}));
    assert!(!err, "{r}");
    assert!(r["input_fixed"].as_array().unwrap().len() >= 5, "{r}");
    // Seen in the run: schema/name beside genome, genome = the body as a string.
    let (r, err) = m.tool("submit_genome", json!({
        "schema": "lifeapi.genome/1", "name": "Round1_BasicSwimmer", "brain": {"hidden": 4},
        "mutation": {"rate": 0.1, "size": 0.2},
        "genome": "[{\"part\":\"membrane\",\"radius\":5},{\"part\":\"flagellum\",\"power\":0.8}]"}));
    assert!(!err, "{r}");
    // Seen in run 3: a sensor without its "part" tag. The error names the item.
    let (t, err) = m.tool("submit_genome", json!({"genome": {"schema": "lifeapi.genome/1", "name": "x",
        "body": [{"part": "membrane", "radius": 5}, {"part": "mouth"}, {"angle": -30, "chemical": "sugar"}],
        "brain": {"hidden": 4}, "mutation": {"rate": 0.1, "size": 0.2}}}));
    assert!(err);
    assert!(t.as_str().unwrap().contains("body[2] has no \"part\""), "{t}");
    // A real rule break is still refused, with the example attached.
    let (t, err) = m.tool("submit_genome", json!({"genome": {"schema": "lifeapi.genome/1", "name": "x",
        "body": [{"part": "membrane", "radius": 99}], "brain": {"hidden": 4}, "mutation": {"rate": 0.1, "size": 0.2}}}));
    assert!(err);
    let t = t.as_str().unwrap();
    assert!(t.contains("radius") && t.contains("A complete, valid submit_genome call"), "{t}");
}

#[test]
fn unreachable_server_is_a_tool_error() {
    let l = bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    drop(l); // nothing listens there now
    let mut m = Mcp::start(&addr);
    let (t, err) = m.tool("catalog", json!({}));
    assert!(err);
    assert!(t.as_str().unwrap().contains("cannot reach"), "{t}");
}

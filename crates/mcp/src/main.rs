//! lifeapi-mcp [--addr 127.0.0.1:4343] [--name NAME]
//!
//! An MCP server over stdio (JSON-RPC 2.0, one message per line) that plays
//! LifeAPI on a running lifeapi-server. Each tool is one API op; the reply is
//! the server's JSON. A refusal comes back as a tool error that names the
//! rule, so a model can correct itself. The connection (and its `hello`) is
//! made on the first tool call and remade if it drops.
//!
//! Hand-written (no MCP crate in the local cache): initialize, ping,
//! tools/list, tools/call. Notifications are accepted and ignored.

use lifeapi_server::bot::Client;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const INSTRUCTIONS: &str = "\
LifeAPI is an evolution sandbox. You design single-celled organisms as JSON \
genomes, release them into a shared pool, and the pool decides what survives: \
organisms sense chemicals, think with a small neural net, swim, eat sugar \
(energy) and mineral (material), divide when they have enough of both, and \
starve when energy runs out. Time only moves when you call step.\n\
A good loop: catalog (rules and costs) -> submit_genome -> release -> step \
(use subscribe so a long step stops early when something happens) -> report \
(what happened, with plain-language notes) -> design a better genome and \
release it as a new lineage. Releasing costs budget, which refills as time \
passes. Every refusal names the rule it broke.";

fn tools() -> Value {
    let genome_schema = json!({
        "type": "object",
        "description": "A lifeapi.genome/1 document. Call catalog for the parts, ranges and costs.",
        "properties": {
            "schema": {"type": "string", "const": "lifeapi.genome/1"},
            "name": {"type": "string"},
            "body": {"type": "array", "items": {"type": "object"},
                     "description": "Parts, each {\"part\": kind, ...fields}. Exactly one membrane."},
            "brain": {"type": "object", "description": "{\"hidden\": 1..16} (weights optional: random per founder)"},
            "mutation": {"type": "object", "description": "{\"rate\": 0..1, \"size\": 0..1}"}
        },
        "required": ["schema", "name", "body", "brain", "mutation"]
    });
    json!([
        {"name": "catalog", "description": "The rules: genome parts with their fields and ranges, how the phenotype and its costs are derived, and the world's constants.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "submit_genome", "description": "Check and register a genome as a new species you own. Returns its id and derived phenotype (capacity, upkeep, brain size), or the exact rule it broke.",
         "inputSchema": {"type": "object", "properties": {"genome": genome_schema}, "required": ["genome"]}},
        {"name": "release", "description": "Release `count` founders of one of your species as a new lineage at random places. Costs budget (5 each).",
         "inputSchema": {"type": "object", "properties": {
             "species": {"type": "integer", "minimum": 0}, "count": {"type": "integer", "minimum": 1, "maximum": 200}},
             "required": ["species", "count"]}},
        {"name": "observe", "description": "Your budget, the world's population, your lineages, and up to `limit` of your organisms (position, heading, energy, age). Use limit 0 for just the summary.",
         "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "minimum": 0, "maximum": 256}}}},
        {"name": "step", "description": "Advance time by up to `ticks` (max 10000). Stops early if one of your subscriptions fires; `woke` says which.",
         "inputSchema": {"type": "object", "properties": {"ticks": {"type": "integer", "minimum": 1, "maximum": 10000}}, "required": ["ticks"]}},
        {"name": "report", "description": "What happened to your lineages since your last report: births, starvations, energy in and out per organism per tick, and plain-language notes on what is limiting each lineage.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "subscribe", "description": "Stop the next step early when: population_below / population_above (lineage, value), extinct (lineage), or tick (value). One-shot.",
         "inputSchema": {"type": "object", "properties": {
             "event": {"type": "string", "enum": ["population_below", "population_above", "extinct", "tick"]},
             "lineage": {"type": "integer"}, "value": {"type": "integer"}},
             "required": ["event"]}},
        {"name": "unsubscribe", "description": "Remove a subscription by id.",
         "inputSchema": {"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}}
    ])
}

const GENOME_FIELDS: [&str; 5] = ["schema", "name", "body", "brain", "mutation"];

const SUBMIT_EXAMPLE: &str = r#"{"genome": {"schema": "lifeapi.genome/1", "name": "MyCell", "body": [{"part": "membrane", "radius": 5}, {"part": "flagellum", "power": 0.7}, {"part": "mouth"}, {"part": "chemo_sensor", "angle": -30, "chemical": "sugar"}, {"part": "chemo_sensor", "angle": 30, "chemical": "sugar"}], "brain": {"hidden": 6}, "mutation": {"rate": 0.05, "size": 0.3}}}"#;

/// If `v` is a string holding JSON, parse it; otherwise return it unchanged.
fn unstring(v: Value) -> Value {
    match &v {
        Value::String(s) if s.trim_start().starts_with(['{', '[']) => serde_json::from_str(s).unwrap_or(v),
        _ => v,
    }
}

/// Undo the formatting slips models make with submit_genome, without
/// changing what the genome says (the server still enforces every rule):
/// - genome fields given beside `genome` instead of inside it are moved in;
/// - JSON sent as a string (the genome, or its body/brain/mutation) is parsed;
/// - a bare array given as the genome is taken as its body.
/// Returns the arguments and a list of what was fixed, for the reply.
pub fn normalize_submit(args: Value) -> (Value, Vec<String>) {
    let mut fixed = Vec::new();
    let Value::Object(mut m) = args else { return (args, fixed) };
    let mut genome = m.remove("genome").map(unstring).unwrap_or(Value::Null);
    if genome.is_array() {
        genome = json!({"body": genome});
        fixed.push("an array given as `genome` was taken as its `body`".into());
    }
    if genome.is_null() {
        genome = json!({});
    }
    if let Value::Object(g) = &mut genome {
        for f in GENOME_FIELDS {
            if let Some(v) = m.remove(f) {
                if !g.contains_key(f) {
                    g.insert(f.to_string(), v);
                    fixed.push(format!("`{f}` moved inside `genome`"));
                }
            }
        }
        for f in ["body", "brain", "mutation"] {
            if let Some(v) = g.get(f) {
                if v.is_string() {
                    let parsed = unstring(v.clone());
                    if !parsed.is_string() {
                        g.insert(f.to_string(), parsed);
                        fixed.push(format!("`{f}` was a JSON string; parsed it"));
                    }
                }
            }
        }
    }
    m.insert("genome".into(), genome);
    (Value::Object(m), fixed)
}

const PARTS: [&str; 6] = ["membrane", "flagellum", "cilia", "mouth", "chemo_sensor", "vacuole"];

/// Point at the exact body item a model got wrong. serde's own message for
/// a tagged item ("missing field `part`", or a field of the wrong variant)
/// doesn't say which item, and small models can't find it.
pub fn check_body(genome: &Value) -> Result<(), String> {
    let Some(body) = genome.get("body").and_then(|b| b.as_array()) else { return Ok(()) };
    for (i, item) in body.iter().enumerate() {
        match item.get("part").and_then(|p| p.as_str()) {
            None => {
                return Err(format!(
                    "body[{i}] has no \"part\" field: {item}. Every body item needs \"part\": one of {}.",
                    PARTS.join(", ")
                ))
            }
            Some(p) if !PARTS.contains(&p) => {
                return Err(format!("body[{i}] has unknown part \"{p}\". Parts are: {}.", PARTS.join(", ")))
            }
            _ => {}
        }
    }
    Ok(())
}

struct Game {
    addr: String,
    name: String,
    client: Option<Client>,
}

impl Game {
    /// Run one tool as one API op. Ok(text) or Err(text) for a tool error.
    fn call(&mut self, tool: &str, args: Value) -> Result<String, String> {
        let known = tools();
        if !known.as_array().unwrap().iter().any(|t| t["name"] == tool) {
            return Err(format!("unknown tool {tool}"));
        }
        if self.client.is_none() {
            let mut c = Client::connect(&self.addr)
                .map_err(|e| format!("cannot reach lifeapi-server at {}: {e}", self.addr))?;
            c.call(json!({"op": "hello", "name": self.name}))?;
            self.client = Some(c);
        }
        let args = unstring(args);
        let mut req = match args {
            Value::Object(m) => Value::Object(m),
            Value::Null => json!({}),
            other => return Err(format!("arguments must be an object, got {other}")),
        };
        let mut fixed = Vec::new();
        if tool == "submit_genome" {
            (req, fixed) = normalize_submit(req);
            if let Err(e) = check_body(&req["genome"]) {
                return Err(format!("submit_genome: {e}\nA complete, valid submit_genome call looks like: {SUBMIT_EXAMPLE}"));
            }
        }
        req["op"] = json!(tool);
        let c = self.client.as_mut().unwrap();
        match c.call(req) {
            Ok(mut v) => {
                if let Some(m) = v.as_object_mut() {
                    m.remove("ok");
                    if !fixed.is_empty() {
                        m.insert("input_fixed".into(), json!(fixed));
                    }
                }
                Ok(v.to_string())
            }
            Err(e) => {
                if e.contains("Broken pipe") || e.contains("bad reply \"\"") || e.contains("Connection reset") {
                    self.client = None;
                }
                if tool == "submit_genome" {
                    Err(format!("{e}\nA complete, valid submit_genome call looks like: {SUBMIT_EXAMPLE}"))
                } else {
                    Err(e)
                }
            }
        }
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

fn handle(g: &mut Game, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone(); // no id: a notification, no reply
    let method = msg["method"].as_str().unwrap_or("");
    Some(match method {
        "initialize" => {
            let version = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
            reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "lifeapi", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => reply(&id, json!({})),
        "tools/list" => reply(&id, json!({"tools": tools()})),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(Value::Null);
            let (text, is_error) = match g.call(name, args) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            reply(&id, json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        _ => error(&id, -32601, &format!("method not found: {method}")),
    })
}

fn main() {
    let mut addr = "127.0.0.1:4343".to_string();
    let mut name = "mcp-agent".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().expect("--addr HOST:PORT"),
            "--name" => name = args.next().expect("--name NAME"),
            other => {
                eprintln!("unknown argument {other}; usage: lifeapi-mcp [--addr HOST:PORT] [--name NAME]");
                std::process::exit(2);
            }
        }
    }
    let mut g = Game { addr, name, client: None };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Err(e) => Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
            Ok(msg) => handle(&mut g, &msg),
        };
        if let Some(r) = resp {
            if writeln!(out, "{r}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
}

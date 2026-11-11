//! The M1 scripted bot: a plain API client with no model behind it. It
//! submits a genome, founds a lineage, steps time, watches the population,
//! and reseeds from its budget if the lineage runs low. Everything it does
//! goes through the public protocol, the same one an LLM agent uses.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

pub const BOT_GENOME: &str = r#"{
  "schema": "lifeapi.genome/1",
  "name": "Sweeper",
  "body": [
    {"part": "membrane", "radius": 5},
    {"part": "flagellum", "power": 0.7},
    {"part": "cilia", "power": 0.6},
    {"part": "mouth"},
    {"part": "chemo_sensor", "angle": -40, "chemical": "sugar"},
    {"part": "chemo_sensor", "angle": 0, "chemical": "sugar"},
    {"part": "chemo_sensor", "angle": 40, "chemical": "sugar"}
  ],
  "brain": {"hidden": 6},
  "mutation": {"rate": 0.05, "size": 0.3}
}"#;

pub struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
}

impl Client {
    pub fn connect(addr: &str) -> std::io::Result<Client> {
        let s = TcpStream::connect(addr)?;
        Ok(Client { w: s.try_clone()?, r: BufReader::new(s) })
    }

    /// Send one request; return the reply, or Err with the server's message.
    pub fn call(&mut self, req: Value) -> Result<Value, String> {
        writeln!(self.w, "{req}").map_err(|e| e.to_string())?;
        let mut line = String::new();
        self.r.read_line(&mut line).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply {line:?}: {e}"))?;
        if v["ok"] == true {
            Ok(v)
        } else {
            Err(v["error"].as_str().unwrap_or("unknown error").to_string())
        }
    }
}

#[derive(Debug)]
pub struct Report {
    pub lineage: u64,
    pub final_tick: u64,
    pub alive: u64,
    pub reseeds: u32,
    /// (tick, alive) after every step.
    pub curve: Vec<(u64, u64)>,
}

pub fn run(addr: &str, target_tick: u64, chunk: u64, verbose: bool) -> Result<Report, String> {
    run_paced(addr, target_tick, chunk, std::time::Duration::ZERO, verbose)
}

/// As `run`, sleeping `pace` after every step so a person can watch.
pub fn run_paced(
    addr: &str,
    target_tick: u64,
    chunk: u64,
    pace: std::time::Duration,
    verbose: bool,
) -> Result<Report, String> {
    let mut c = Client::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
    c.call(json!({"op": "hello", "name": "scripted-bot"}))?;
    let cat = c.call(json!({"op": "catalog"}))?;
    if cat["genome"]["schema"] != "lifeapi.genome/1" {
        return Err(format!("unexpected genome schema {}", cat["genome"]["schema"]));
    }
    let genome: Value = serde_json::from_str(BOT_GENOME).expect("BOT_GENOME is JSON");
    let r = c.call(json!({"op": "submit_genome", "genome": genome}))?;
    let species = r["species"].as_u64().ok_or("no species id")?;
    let r = c.call(json!({"op": "release", "species": species, "count": 60}))?;
    let lineage = r["lineage"].as_u64().ok_or("no lineage id")?;
    let mut reseeds = 0;
    let mut curve = Vec::new();
    let mut tick = r["tick"].as_u64().unwrap_or(0);
    let mut alive = 60;
    while tick < target_tick {
        let n = chunk.min(target_tick - tick);
        let r = c.call(json!({"op": "step", "ticks": n}))?;
        tick = r["tick"].as_u64().unwrap_or(tick);
        let obs = c.call(json!({"op": "observe", "limit": 0}))?;
        alive = obs["lineages"]
            .as_array()
            .and_then(|ls| ls.iter().find(|l| l["lineage"] == lineage))
            .and_then(|l| l["alive"].as_u64())
            .unwrap_or(0);
        curve.push((tick, alive));
        if !pace.is_zero() {
            std::thread::sleep(pace);
        }
        if verbose && (pace.is_zero() || tick % 500 < n) {
            println!(
                "tick {tick:>6}  alive {alive:>4}  budget {:>6.1}  pool {}",
                obs["budget"].as_f64().unwrap_or(0.0),
                obs["world"]["population"]
            );
        }
        // Reseed a dwindling lineage into the same species, if we can afford it.
        if alive < 20 && obs["budget"].as_f64().unwrap_or(0.0) >= 150.0 {
            c.call(json!({"op": "release", "species": species, "count": 30}))?;
            reseeds += 1;
        }
    }
    Ok(Report { lineage, final_tick: tick, alive, reseeds, curve })
}

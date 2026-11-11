//! LifeAPI server, protocol 1 (M1: sandbox).
//!
//! Transport: newline-delimited JSON over TCP, loopback only. One request per
//! line, one reply per line, in order.
//!
//! Threading: a single simulation thread owns the World and the player
//! table. Connection threads only read lines and hand them over a channel,
//! so every command is applied in one total order, the order it arrived in.
//! That order is what a future replay log records.

pub mod bot;
pub mod snapshot;
pub mod web;

use lifeapi_sim::genome::{self, Genome};
use lifeapi_sim::World;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

pub const PROTOCOL: u32 = 1;
pub const MAX_LINE_BYTES: usize = 1 << 20;
pub const MAX_STEP: u64 = 10_000;
pub const MAX_RELEASE: usize = 200;
pub const MAX_OBSERVE: usize = 256;
pub const BUDGET_START: f64 = 500.0;
pub const BUDGET_CAP: f64 = 1000.0;
pub const BUDGET_PER_TICK: f64 = 0.1;
pub const RELEASE_COST_EACH: f64 = 5.0;

#[derive(Deserialize, Debug)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Hello { name: String },
    Catalog,
    SubmitGenome { genome: Value },
    Release { species: u32, count: usize },
    Observe {
        #[serde(default)]
        limit: Option<usize>,
    },
    Step { ticks: u64 },
}

struct Player {
    name: String,
    budget: f64,
}

/// One point of the population graph.
#[derive(Clone, Debug, serde::Serialize)]
pub struct HistPoint {
    pub tick: u64,
    pub total: u32,
    /// (lineage, alive), lineages with at least one organism.
    pub lineages: Vec<(u32, u32)>,
}

pub const HISTORY_MAX: usize = 2000;

/// The server's whole state; lives on the simulation thread only.
pub struct Game {
    world: World,
    players: Vec<Player>,
    /// species id -> owning player
    species_owner: Vec<usize>,
    /// lineage id -> owning player (lineage ids start at 1; index 0 unused)
    lineage_owner: Vec<Option<usize>>,
    /// Bumped by every command that changes the world, so spectators know
    /// when there's something new to send.
    version: u64,
    history: Vec<HistPoint>,
    hist_every: u64,
}

impl Game {
    pub fn new(world: World) -> Game {
        let n = world.species.len();
        let mut g = Game {
            world,
            players: Vec::new(),
            species_owner: vec![usize::MAX; n],
            lineage_owner: vec![None],
            version: 0,
            history: Vec::new(),
            hist_every: 50,
        };
        g.sample_history();
        g
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn history(&self) -> &[HistPoint] {
        &self.history
    }

    /// Live organisms per lineage id (index = lineage id).
    pub fn lineage_counts(&self) -> Vec<u32> {
        let mut counts = vec![0u32; self.lineage_owner.len().max(1)];
        for o in &self.world.orgs {
            let l = o.lineage as usize;
            if l >= counts.len() {
                counts.resize(l + 1, 0);
            }
            counts[l] += 1;
        }
        counts
    }

    fn sample_history(&mut self) {
        let counts = self.lineage_counts();
        let lineages = counts
            .iter()
            .enumerate()
            .filter(|(_, &c)| c > 0)
            .map(|(l, &c)| (l as u32, c))
            .collect();
        self.history.push(HistPoint { tick: self.world.tick, total: self.world.orgs.len() as u32, lineages });
        if self.history.len() > HISTORY_MAX {
            // Halve the resolution: keep every other point, sample half as often.
            let mut i = 0;
            self.history.retain(|_| {
                i += 1;
                i % 2 == 1
            });
            self.hist_every *= 2;
        }
    }

    pub fn owner_name(&self, lineage: u32) -> Option<&str> {
        self.lineage_owner
            .get(lineage as usize)
            .copied()
            .flatten()
            .map(|p| self.players[p].name.as_str())
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Apply one request line from a connection. Returns the reply line and
    /// the connection's player afterwards (set by `hello`).
    pub fn handle(&mut self, player: Option<usize>, line: &str) -> (String, Option<usize>) {
        let mut player = player;
        let reply = match serde_json::from_str::<Request>(line) {
            Err(e) => self.err(format!(
                "bad request: {e}. Ops: hello{{name}}, catalog, submit_genome{{genome}}, \
                 release{{species,count}}, observe{{limit?}}, step{{ticks}}"
            )),
            Ok(Request::Hello { name }) => {
                if player.is_some() {
                    self.err("hello: this connection already has a player".into())
                } else if name.is_empty() || name.len() > 64 {
                    self.err("hello: name must be 1..=64 bytes".into())
                } else {
                    self.players.push(Player { name, budget: BUDGET_START });
                    let id = self.players.len() - 1;
                    player = Some(id);
                    self.ok(json!({
                        "player": id,
                        "protocol": PROTOCOL,
                        "budget": BUDGET_START,
                        "limits": {
                            "max_line_bytes": MAX_LINE_BYTES,
                            "max_step": MAX_STEP,
                            "max_release": MAX_RELEASE,
                            "max_observe": MAX_OBSERVE,
                            "budget_cap": BUDGET_CAP,
                            "budget_per_tick": BUDGET_PER_TICK,
                            "release_cost_each": RELEASE_COST_EACH,
                        },
                    }))
                }
            }
            Ok(req) => match player {
                None => self.err("say hello first: {\"op\":\"hello\",\"name\":\"...\"}".into()),
                Some(p) => self.handle_player(p, req),
            },
        };
        (reply, player)
    }

    fn handle_player(&mut self, p: usize, req: Request) -> String {
        match req {
            Request::Hello { .. } => unreachable!("handled above"),
            Request::Catalog => {
                let c = &self.world.cfg;
                self.ok(json!({
                    "genome": genome::catalog(),
                    "world": {
                        "width": self.world.grid.world_w(),
                        "height": self.world.grid.world_h(),
                        "cell": c.cell,
                        "torus": true,
                        "eat_rate_per_mouth": c.eat_rate,
                        "sugar_energy": c.sugar_energy,
                        "move_cost": c.move_cost,
                        "max_speed": c.max_speed,
                        "split_frac": c.split_frac,
                        "material_cap": c.material_cap,
                        "max_population": c.max_population,
                    },
                }))
            }
            Request::SubmitGenome { genome } => {
                let g: Genome = match serde_json::from_value(genome) {
                    Ok(g) => g,
                    Err(e) => return self.err(format!("submit_genome: not a valid {} document: {e}", genome::SCHEMA)),
                };
                match self.world.add_species(g) {
                    Err(rule) => self.err(format!("submit_genome: {rule}")),
                    Ok(id) => {
                        self.species_owner.push(p);
                        self.version += 1;
                        let ph = &self.world.species[id as usize].phen;
                        let reply = json!({
                            "species": id,
                            "phenotype": {
                                "radius": ph.radius,
                                "capacity": ph.capacity,
                                "upkeep_per_tick": ph.upkeep,
                                "turn_rad_per_tick": ph.turn,
                                "child_material": ph.child_material,
                                "brain_inputs": ph.n_in,
                                "brain_weights": ph.n_weights,
                            },
                        });
                        self.ok(reply)
                    }
                }
            }
            Request::Release { species, count } => {
                if self.species_owner.get(species as usize) != Some(&p) {
                    return self.err(format!("release: species {species} is not yours (submit_genome first)"));
                }
                if count == 0 || count > MAX_RELEASE {
                    return self.err(format!("release: count must be 1..={MAX_RELEASE}"));
                }
                let room = self.world.cfg.max_population.saturating_sub(self.world.orgs.len());
                if count > room {
                    return self.err(format!("release: the pool has room for {room} more organisms"));
                }
                let cost = count as f64 * RELEASE_COST_EACH;
                if self.players[p].budget < cost {
                    return self.err(format!(
                        "release: costs {cost} ({RELEASE_COST_EACH} each), budget is {:.1}",
                        self.players[p].budget
                    ));
                }
                let lineage = self.world.release(species, count).expect("species checked above");
                self.players[p].budget -= cost;
                if self.lineage_owner.len() <= lineage as usize {
                    self.lineage_owner.resize(lineage as usize + 1, None);
                }
                self.lineage_owner[lineage as usize] = Some(p);
                self.version += 1;
                let budget = self.players[p].budget;
                self.ok(json!({"lineage": lineage, "cost": cost, "budget": budget}))
            }
            Request::Observe { limit } => {
                let limit = limit.unwrap_or(MAX_OBSERVE).min(MAX_OBSERVE);
                let mut lineages: Vec<(u32, u32, usize, f64, u32)> = Vec::new(); // id, species, count, energy sum, oldest
                let mut organisms = Vec::new();
                for o in &self.world.orgs {
                    if self.lineage_owner.get(o.lineage as usize).copied().flatten() != Some(p) {
                        continue;
                    }
                    match lineages.iter_mut().find(|l| l.0 == o.lineage) {
                        Some(l) => {
                            l.2 += 1;
                            l.3 += o.energy as f64;
                            l.4 = l.4.max(o.age);
                        }
                        None => lineages.push((o.lineage, o.species, 1, o.energy as f64, o.age)),
                    }
                    if organisms.len() < limit {
                        organisms.push(json!({
                            "id": o.id, "lineage": o.lineage,
                            "x": o.x, "y": o.y, "heading": o.heading,
                            "energy": o.energy, "material": o.material, "age": o.age,
                        }));
                    }
                }
                lineages.sort_by_key(|l| l.0);
                let mine: Vec<u32> = self
                    .lineage_owner
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| **o == Some(p))
                    .map(|(i, _)| i as u32)
                    .collect();
                let lineage_json: Vec<Value> = mine
                    .iter()
                    .map(|&id| match lineages.iter().find(|l| l.0 == id) {
                        Some(l) => json!({"lineage": id, "species": l.1, "alive": l.2,
                                          "mean_energy": l.3 / l.2 as f64, "oldest_age": l.4}),
                        None => json!({"lineage": id, "alive": 0, "extinct": true}),
                    })
                    .collect();
                self.ok(json!({
                    "budget": self.players[p].budget,
                    "world": {"population": self.world.orgs.len(),
                              "births": self.world.stats.births, "deaths": self.world.stats.deaths},
                    "lineages": lineage_json,
                    "organisms": organisms,
                }))
            }
            Request::Step { ticks } => {
                if ticks == 0 || ticks > MAX_STEP {
                    return self.err(format!("step: ticks must be 1..={MAX_STEP}"));
                }
                for _ in 0..ticks {
                    self.world.tick();
                    if self.world.tick % self.hist_every == 0 {
                        self.sample_history();
                    }
                }
                self.version += 1;
                for pl in self.players.iter_mut() {
                    pl.budget = (pl.budget + BUDGET_PER_TICK * ticks as f64).min(BUDGET_CAP);
                }
                self.ok(json!({"stepped": ticks, "population": self.world.orgs.len()}))
            }
        }
    }

    fn ok(&self, mut body: Value) -> String {
        body["ok"] = json!(true);
        body["tick"] = json!(self.world.tick);
        body.to_string()
    }

    fn err(&self, msg: String) -> String {
        json!({"ok": false, "tick": self.world.tick, "error": msg}).to_string()
    }

    pub fn player_name(&self, p: usize) -> &str {
        &self.players[p].name
    }
}

/// What connection threads send the simulation thread.
pub(crate) enum Msg {
    /// One API request line from a player connection.
    Line {
        player: Option<usize>,
        line: String,
        reply: mpsc::Sender<(String, Option<usize>)>,
    },
    /// A spectator asking for the current view, if it changed since
    /// `since_version`. `history_after`: send history points after this tick
    /// (None = the whole history, as on a fresh connection).
    Snapshot {
        since_version: Option<u64>,
        history_after: Option<u64>,
        reply: mpsc::Sender<Option<snapshot::Snapshot>>,
    },
}

fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Bind and serve forever. Refuses any non-loopback address: there is no
/// authentication yet (design doc §8), so remote players go through an SSH
/// tunnel.
pub fn bind(addr: &str) -> std::io::Result<TcpListener> {
    let l = TcpListener::bind(addr)?;
    let local = l.local_addr()?;
    if !is_loopback(&local) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{local} is not loopback; no auth yet, so use an SSH tunnel"),
        ));
    }
    Ok(l)
}

/// Run the API server on an already-bound listener. Blocks forever.
pub fn serve(listener: TcpListener, game: Game) {
    serve_with_viewer(listener, None, game)
}

/// The API server plus, optionally, the viewer (HTTP page + spectator
/// WebSocket) on a second listener. Blocks forever.
pub fn serve_with_viewer(api: TcpListener, viewer: Option<TcpListener>, game: Game) {
    let (tx, rx) = mpsc::channel::<Msg>();
    thread::spawn(move || sim_thread(game, rx));
    if let Some(v) = viewer {
        let tx = tx.clone();
        thread::spawn(move || web::serve(v, tx));
    }
    for stream in api.incoming() {
        let Ok(stream) = stream else { continue };
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = connection(stream, tx);
        });
    }
}

fn sim_thread(mut game: Game, rx: mpsc::Receiver<Msg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Line { player, line, reply } => {
                let _ = reply.send(game.handle(player, &line));
            }
            Msg::Snapshot { since_version, history_after, reply } => {
                let snap = if since_version == Some(game.version()) {
                    None
                } else {
                    Some(snapshot::build(&game, history_after))
                };
                let _ = reply.send(snap);
            }
        }
    }
}

fn connection(stream: TcpStream, tx: mpsc::Sender<Msg>) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut player = None;
    let (rtx, rrx) = mpsc::channel();
    loop {
        let mut line = String::new();
        let n = reader.by_ref().take(MAX_LINE_BYTES as u64 + 1).read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        if n > MAX_LINE_BYTES {
            let msg = json!({"ok": false, "error": format!("line longer than {MAX_LINE_BYTES} bytes; closing")});
            writeln!(writer, "{msg}")?;
            return Ok(());
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if tx.send(Msg::Line { player, line: line.to_string(), reply: rtx.clone() }).is_err() {
            return Ok(());
        }
        let Ok((reply, p)) = rrx.recv() else { return Ok(()) };
        player = p;
        writeln!(writer, "{reply}")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifeapi_sim::scenario;

    fn game() -> Game {
        Game::new(scenario::pool(scenario::base_config(1, 64, 64), 4, 2))
    }

    fn call(g: &mut Game, p: Option<usize>, line: &str) -> (Value, Option<usize>) {
        let (s, p) = g.handle(p, line);
        (serde_json::from_str(&s).unwrap(), p)
    }

    #[test]
    fn hello_first_then_ops() {
        let mut g = game();
        let (r, p) = call(&mut g, None, r#"{"op":"catalog"}"#);
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("hello"));
        assert!(p.is_none());
        let (r, p) = call(&mut g, None, r#"{"op":"hello","name":"t"}"#);
        assert_eq!(r["ok"], true);
        assert_eq!(r["protocol"], PROTOCOL);
        let (r, _) = call(&mut g, p, r#"{"op":"catalog"}"#);
        assert_eq!(r["genome"]["schema"], genome::SCHEMA);
    }

    #[test]
    fn unknown_fields_and_ops_are_refused_by_name() {
        let mut g = game();
        let (_, p) = call(&mut g, None, r#"{"op":"hello","name":"t"}"#);
        let (r, _) = call(&mut g, p, r#"{"op":"step","tick":5}"#);
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("tick"), "{r}");
        let (r, _) = call(&mut g, p, r#"{"op":"fly"}"#);
        assert!(r["error"].as_str().unwrap().contains("fly"), "{r}");
    }

    #[test]
    fn genome_refusal_names_the_rule_and_ownership_holds() {
        let mut g = game();
        let (_, a) = call(&mut g, None, r#"{"op":"hello","name":"a"}"#);
        let (_, b) = call(&mut g, None, r#"{"op":"hello","name":"b"}"#);
        let bad = scenario::FOUNDER.replace("\"radius\":6", "\"radius\":99");
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"submit_genome","genome":{bad}}}"#));
        assert!(r["error"].as_str().unwrap().contains("radius"), "{r}");
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"submit_genome","genome":{}}}"#, scenario::FOUNDER));
        let sp = r["species"].as_u64().unwrap();
        let (r, _) = call(&mut g, b, &format!(r#"{{"op":"release","species":{sp},"count":5}}"#));
        assert!(r["error"].as_str().unwrap().contains("not yours"), "{r}");
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"release","species":{sp},"count":5}}"#));
        assert_eq!(r["ok"], true, "{r}");
        // b sees none of a's organisms (fog of war); a sees all five.
        let (r, _) = call(&mut g, b, r#"{"op":"observe"}"#);
        assert_eq!(r["organisms"].as_array().unwrap().len(), 0);
        let (r, _) = call(&mut g, a, r#"{"op":"observe"}"#);
        assert_eq!(r["organisms"].as_array().unwrap().len(), 5);
        assert_eq!(r["lineages"][0]["alive"], 5);
    }

    #[test]
    fn budget_limits_release_and_refills_with_time() {
        let mut g = game();
        let (_, a) = call(&mut g, None, r#"{"op":"hello","name":"a"}"#);
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"submit_genome","genome":{}}}"#, scenario::FOUNDER));
        let sp = r["species"].as_u64().unwrap();
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"release","species":{sp},"count":100}}"#));
        assert_eq!(r["budget"], 0.0, "{r}");
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"release","species":{sp},"count":1}}"#));
        assert!(r["error"].as_str().unwrap().contains("budget"), "{r}");
        call(&mut g, a, r#"{"op":"step","ticks":50}"#);
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"release","species":{sp},"count":1}}"#));
        assert_eq!(r["ok"], true, "{r}");
    }

    #[test]
    fn time_moves_only_on_step() {
        let mut g = game();
        let (_, a) = call(&mut g, None, r#"{"op":"hello","name":"a"}"#);
        for line in [r#"{"op":"catalog"}"#, r#"{"op":"observe"}"#] {
            let (r, _) = call(&mut g, a, line);
            assert_eq!(r["tick"], 0);
        }
        let (r, _) = call(&mut g, a, r#"{"op":"step","ticks":7}"#);
        assert_eq!(r["tick"], 7);
    }

    #[test]
    fn non_loopback_bind_is_refused() {
        // 0.0.0.0 binds all interfaces, which is not loopback.
        let e = bind("0.0.0.0:0").unwrap_err();
        assert!(e.to_string().contains("not loopback"), "{e}");
    }
}

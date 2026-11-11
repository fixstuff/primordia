//! LifeAPI server, protocol 1 (M1: sandbox).
//!
//! Transport: newline-delimited JSON over TCP, loopback only. One request per
//! line, one reply per line, in order.
//!
//! Threading: a single simulation thread owns the World and the player
//! table. Connection threads only read lines and hand them over a channel,
//! so every command is applied in one total order, the order it arrived in.
//! That order is what a future replay log records.

pub mod arena;
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

/// Budget per founder: 5 for a radius-5 body, scaling with body area, so
/// biomass can't be bought cheaply (arena match 1: a radius-20 founder for a
/// flat 5 was 400 biomass that lived ~2000 ticks on its starting energy).
pub fn founder_cost(radius: f32) -> f64 {
    let r = radius as f64;
    RELEASE_COST_EACH * (r / 5.0) * (r / 5.0)
}

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
    Report,
    Subscribe {
        event: Event,
        #[serde(default)]
        lineage: Option<u32>,
        #[serde(default)]
        value: Option<u64>,
    },
    Unsubscribe { id: u64 },
    /// Arena only: done for this window; the reply comes when the next opens.
    EndTurn,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// Lineage has fewer than `value` alive.
    PopulationBelow,
    /// Lineage has more than `value` alive.
    PopulationAbove,
    /// Lineage has none alive.
    Extinct,
    /// The world reached tick `value`.
    Tick,
}

#[derive(Clone, Debug)]
struct Sub {
    id: u64,
    event: Event,
    lineage: u32,
    value: u64,
}

struct Player {
    name: String,
    budget: f64,
    /// Lineage counters at the last `report`, to report the difference.
    last_report: Vec<(u32, lifeapi_sim::world::LineageStats)>,
    last_report_tick: u64,
    subs: Vec<Sub>,
}

pub const MAX_SUBSCRIPTIONS: usize = 32;

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
    next_sub: u64,
    arena: Option<arena::Arena>,
    /// What `catalog` says about the world (name, description, spec).
    world_info: Value,
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
            next_sub: 0,
            arena: None,
            world_info: json!({"name": "calm"}),
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

    /// This match's combat setting, told to every player at hello.
    pub fn combat_rules(&self) -> Value {
        use lifeapi_sim::world::CombatMode;
        let mode = self.world.cfg.combat;
        json!({
            "mode": mode.name(),
            "means": match mode {
                CombatMode::Off => "no combat: combat parts are refused",
                CombatMode::Colony => "colony: your own organisms never fight or smell each other; \
                                       you fight other players' organisms (like rival ant colonies)",
                CombatMode::Individual => "individual: every organism for itself; jaws and stingers bite \
                                           ANY organism, your own lineage included, and sensors smell everyone",
            },
            "parts": "see catalog: jaw, stinger (offence); shell, spikes, camouflage, toxin (defence); \
                      chemo_sensor chemical \"organism\" smells bodies",
            "body_evolution": if self.world.cfg.body_mutation > 0.0 {
                format!("ON: each birth has a {:.0}% chance the child's BODY mutates (a part nudged, added or \
                         removed); a changed body is a new species in your lineage, still yours. Selection decides \
                         which bodies spread. Brains mutate as your genome's mutation settings say.",
                        100.0 * self.world.cfg.body_mutation)
            } else {
                "off: bodies never change; only brains mutate".to_string()
            },
        })
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
        let (reply, p) = self.handle_deferrable(player, line);
        (reply.unwrap_or_default(), p)
    }

    /// Describe the world in `catalog` (set by whoever built the world).
    /// Every player's name, in join order (for the viewer's title screen).
    pub fn player_names(&self) -> Vec<String> {
        self.players.iter().map(|p| p.name.clone()).collect()
    }

    /// The world spec this game was built from (name, seed, dynamics).
    pub fn world_info(&self) -> &Value {
        &self.world_info
    }

    pub fn set_world_info(&mut self, info: Value) {
        self.world_info = info;
    }

    /// As `handle`, but an arena `end_turn` returns None: its reply is sent
    /// later, by `arena_poll`, when the next window opens.
    pub fn handle_deferrable(&mut self, player: Option<usize>, line: &str) -> (Option<String>, Option<usize>) {
        if let (Some(a), Some(p)) = (self.arena.as_mut(), player) {
            a.log_line(p, self.world.tick, line);
        }
        if let Ok(Request::EndTurn) = serde_json::from_str::<Request>(line) {
            return match (player, self.arena.is_some()) {
                (None, _) => (Some(self.err("say hello first".into())), player),
                (_, false) => (Some(self.err("end_turn is for arena matches; in a sandbox, use step".into())), player),
                (Some(p), true) => (self.arena_end_turn(p), player),
            };
        }
        let mut player = player;
        let reply = match serde_json::from_str::<Request>(line) {
            Err(e) => self.err(format!(
                "bad request: {e}. Ops: hello{{name}}, catalog, submit_genome{{genome}}, \
                 release{{species,count}}, observe{{limit?}}, step{{ticks}}, report, \
                 subscribe{{event,lineage?,value?}}, unsubscribe{{id}}"
            )),
            Ok(Request::Hello { name }) => {
                if player.is_some() {
                    self.err("hello: this connection already has a player".into())
                } else if name.is_empty() || name.len() > 64 {
                    self.err("hello: name must be 1..=64 bytes".into())
                } else if self.arena.as_ref().is_some_and(|a| self.players.len() >= a.cfg.players) {
                    // A full arena lets a player whose connection dropped come
                    // back by name and take its slot again (a crashed agent
                    // runner must not stall every later window). Names are not
                    // authenticated yet: loopback only, like everything else.
                    match self.players.iter().position(|pl| pl.name == name) {
                        Some(id) => {
                            player = Some(id);
                            let budget = self.players[id].budget;
                            self.ok(json!({"player": id, "rejoined": true, "protocol": PROTOCOL, "budget": budget,
                                           "arena": self.arena.as_ref().map(|a| a.rules())}))
                        }
                        None => self.err("hello: this arena match is full".into()),
                    }
                } else {
                    self.players.push(Player {
                        name,
                        budget: BUDGET_START,
                        last_report: Vec::new(),
                        last_report_tick: self.world.tick,
                        subs: Vec::new(),
                    });
                    let id = self.players.len() - 1;
                    player = Some(id);
                    let arena = self.arena_joined();
                    self.ok(json!({
                        "arena": arena,
                        "combat": self.combat_rules(),
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
                            "release_cost_each": "5 * (radius/5)^2 per founder: radius 2 = 0.8, 5 = 5, 10 = 20, 20 = 80",
                        },
                    }))
                }
            }
            Ok(req) => match player {
                None => self.err("say hello first: {\"op\":\"hello\",\"name\":\"...\"}".into()),
                Some(p) => match self.arena_refuses(&req) {
                    Some(e) => self.err(e),
                    None => self.handle_player(p, req),
                },
            },
        };
        // A hello arrives before the connection has a player, so log it once
        // it has one: the match log must say which index is which name.
        if let (Some(a), Some(p)) = (self.arena.as_mut(), player) {
            if line.contains("\"hello\"") {
                a.log_line(p, self.world.tick, line);
            }
        }
        (Some(reply), player)
    }

    fn handle_player(&mut self, p: usize, req: Request) -> String {
        match req {
            Request::Hello { .. } | Request::EndTurn => unreachable!("handled above"),
            Request::Catalog => {
                let c = &self.world.cfg;
                self.ok(json!({
                    "genome": genome::catalog(),
                    "combat_setting": self.combat_rules(),
                    "world_spec": self.world_info,
                    "arena": self.arena.as_ref().map(|a| a.rules()),
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
                if self.world.cfg.combat == lifeapi_sim::world::CombatMode::Off {
                    if let Some(i) = g.body.iter().position(|p| {
                        matches!(
                            p,
                            genome::Part::Jaw { .. }
                                | genome::Part::Stinger { .. }
                                | genome::Part::Shell { .. }
                                | genome::Part::Spikes { .. }
                                | genome::Part::Camouflage
                                | genome::Part::Toxin { .. }
                        )
                    }) {
                        return self.err(format!(
                            "submit_genome: body[{i}] is a combat part, and this match has combat off. \
                             Remove jaw, stinger, shell, spikes, camouflage and toxin."
                        ));
                    }
                }
                match self.world.add_species(g) {
                    Err(rule) => self.err(format!("submit_genome: {rule}")),
                    Ok(id) => {
                        // By id: the sim also makes species of its own (evolved
                        // bodies), so ids don't follow submission order.
                        if self.species_owner.len() <= id as usize {
                            self.species_owner.resize(id as usize + 1, usize::MAX);
                        }
                        self.species_owner[id as usize] = p;
                        // One team per player: your own organisms never fight.
                        self.world.species[id as usize].team = p as u32 + 1;
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
                let each = founder_cost(self.world.species[species as usize].phen.radius);
                let cost = count as f64 * each;
                if self.players[p].budget < cost {
                    return self.err(format!(
                        "release: costs {cost:.1} ({each:.1} each = 5 x (radius/5)^2), budget is {:.1}",
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
                let mut stepped = 0;
                let mut woke = Vec::new();
                while stepped < ticks {
                    self.world.tick();
                    stepped += 1;
                    if self.world.tick % self.hist_every == 0 {
                        self.sample_history();
                    }
                    woke = self.fired(p);
                    if !woke.is_empty() {
                        break;
                    }
                }
                self.version += 1;
                for pl in self.players.iter_mut() {
                    pl.budget = (pl.budget + BUDGET_PER_TICK * stepped as f64).min(BUDGET_CAP);
                }
                let woke_ids: Vec<u64> = woke.iter().map(|s| s.id).collect();
                self.players[p].subs.retain(|s| !woke_ids.contains(&s.id));
                let woke: Vec<Value> = woke
                    .iter()
                    .map(|s| json!({"id": s.id, "event": s.event, "lineage": s.lineage, "value": s.value}))
                    .collect();
                self.ok(json!({"stepped": stepped, "population": self.world.orgs.len(), "woke": woke}))
            }
            Request::Report => self.report(p),
            Request::Subscribe { event, lineage, value } => {
                if self.players[p].subs.len() >= MAX_SUBSCRIPTIONS {
                    return self.err(format!("subscribe: at most {MAX_SUBSCRIPTIONS} subscriptions"));
                }
                let needs_lineage = event != Event::Tick;
                let lineage = match (needs_lineage, lineage) {
                    (true, None) => return self.err("subscribe: this event needs a lineage".into()),
                    (true, Some(l)) if self.lineage_owner.get(l as usize).copied().flatten() != Some(p) => {
                        return self.err(format!("subscribe: lineage {l} is not yours"))
                    }
                    (_, l) => l.unwrap_or(0),
                };
                let value = match (event, value) {
                    (Event::Extinct, _) => 0,
                    (_, Some(v)) => v,
                    (_, None) => return self.err("subscribe: this event needs a value".into()),
                };
                if event == Event::Tick && value <= self.world.tick {
                    return self.err(format!("subscribe: tick {value} is not in the future (now {})", self.world.tick));
                }
                self.next_sub += 1;
                let id = self.next_sub;
                let sub = Sub { id, event, lineage, value };
                let now = self.sub_true(&sub);
                self.players[p].subs.push(sub);
                self.ok(json!({"id": id, "true_now": now,
                    "note": "one-shot: a step stops at the end of the first tick this is true, and the subscription is removed"}))
            }
            Request::Unsubscribe { id } => {
                let before = self.players[p].subs.len();
                self.players[p].subs.retain(|s| s.id != id);
                if self.players[p].subs.len() == before {
                    self.err(format!("unsubscribe: no subscription {id}"))
                } else {
                    self.ok(json!({"removed": id}))
                }
            }
        }
    }

    fn alive(&self, lineage: u32) -> u64 {
        self.world.lineages.get(lineage as usize).map_or(0, |l| l.alive as u64)
    }

    fn sub_true(&self, s: &Sub) -> bool {
        match s.event {
            Event::PopulationBelow => self.alive(s.lineage) < s.value,
            Event::PopulationAbove => self.alive(s.lineage) > s.value,
            Event::Extinct => self.alive(s.lineage) == 0,
            Event::Tick => self.world.tick >= s.value,
        }
    }

    /// The stepping player's subscriptions that are true now.
    fn fired(&self, p: usize) -> Vec<Sub> {
        self.players[p].subs.iter().filter(|s| self.sub_true(s)).cloned().collect()
    }

    /// What happened to the player's lineages since their last report,
    /// as numbers plus plain sentences written for a model to read.
    fn report(&mut self, p: usize) -> String {
        let now = self.world.tick;
        let since = self.players[p].last_report_tick;
        let mine: Vec<u32> = self
            .lineage_owner
            .iter()
            .enumerate()
            .filter(|(_, o)| **o == Some(p))
            .map(|(i, _)| i as u32)
            .collect();
        // Current energy and material per lineage, from the living.
        let mut cur: Vec<(f64, f64, u32)> = vec![(0.0, 0.0, 0); self.world.lineages.len()];
        for o in &self.world.orgs {
            let ph = &self.world.species[o.species as usize].phen;
            let c = &mut cur[o.lineage as usize];
            c.0 += (o.energy / ph.capacity) as f64;
            c.1 += (o.material / ph.child_material) as f64;
            c.2 += 1;
        }
        let split = self.world.cfg.split_frac as f64;
        let mut lineages = Vec::new();
        let mut notes = Vec::new();
        let mut snapshot = Vec::new();
        for &l in &mine {
            let now_s = self.world.lineages[l as usize].clone();
            let before = self.players[p]
                .last_report
                .iter()
                .find(|(id, _)| *id == l)
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            snapshot.push((l, now_s.clone()));
            let d = |a: u64, b: u64| a.saturating_sub(b);
            let born = d(now_s.born, before.born);
            let starved = d(now_s.starved, before.starved);
            let killed = d(now_s.killed, before.killed);
            let released = d(now_s.released, before.released);
            let ot = d(now_s.org_ticks, before.org_ticks);
            let per = |a: f64, b: f64| if ot == 0 { 0.0 } else { (a - b) / ot as f64 };
            let e_in = per(now_s.energy_in, before.energy_in);
            let upkeep = per(now_s.upkeep, before.upkeep);
            let movement = per(now_s.movement, before.movement);
            let speed = per(now_s.distance, before.distance);
            let prey_in = per(now_s.prey_in, before.prey_in);
            let lost = per(now_s.lost_to_attack, before.lost_to_attack);
            let hurt = per(now_s.hurt, before.hurt);
            let net = e_in + prey_in - upkeep - movement - lost - hurt;
            let age_at_death = if starved == 0 {
                None
            } else {
                Some(d(now_s.age_at_death_sum, before.age_at_death_sum) as f64 / starved as f64)
            };
            let (es, ms, n) = cur[l as usize];
            let mean_energy = if n == 0 { None } else { Some(es / n as f64) };
            let mean_material = if n == 0 { None } else { Some(ms / n as f64) };
            let alive_before = before.alive as i64;
            let species = now_s.species;
            let ph = self.world.species[species as usize].phen.clone();
            let name = Some(self.world.species[species as usize].genome.name.clone());
            lineages.push(json!({
                "lineage": l, "species": species, "genome_name": name,
                "body": {
                    "radius": ph.radius, "mouths": ph.mouths, "flagellum_power": ph.thrust,
                    "chemo_sensors": ph.sensors.len(), "capacity": ph.capacity,
                    "upkeep_per_tick": ph.upkeep, "child_material": ph.child_material,
                    "jaw": ph.jaw, "stinger": ph.stinger, "shell": ph.shell, "spikes": ph.spikes,
                    "camouflage": ph.camouflage, "toxin": ph.toxin,
                },
                "alive": now_s.alive, "alive_at_last_report": before.alive,
                "released": released, "born": born, "starved": starved, "killed_by_enemies": killed,
                "mean_age_at_death": age_at_death,
                "per_organism_per_tick": {
                    "energy_in": e_in, "prey_in": prey_in, "lost_to_attacks": lost, "hurt_by_spikes_or_toxin": hurt,
                    "upkeep": upkeep, "movement": movement, "net": net, "speed": speed,
                },
                "births_per_1000_organism_ticks": if ot == 0 { 0.0 } else { born as f64 * 1000.0 / ot as f64 },
                "mean_energy_fraction_now": mean_energy,
                "mean_material_fraction_of_child_now": mean_material,
            }));
            let tag = format!("Lineage {l}{}", name.as_deref().map(|n| format!(" ({n})")).unwrap_or_default());
            let reported = now_s.alive > 0 || alive_before > 0 || released > 0;
            // Facts about the body that decide its fate, stated plainly.
            if reported && killed > 0 {
                notes.push(format!(
                    "{tag}: {killed} killed by other players' organisms since the last report (losing {lost:.3} energy per \
                     organism per tick to attacks). Defences: shell (halves jaws), spikes (hurt attackers), toxin \
                     (poisons them), camouflage (can't be smelled), or speed."
                ));
            }
            if reported && hurt > prey_in && hurt > 0.0 {
                notes.push(format!(
                    "{tag}'s attacks cost more than they feed: {hurt:.3} energy per organism per tick lost to spikes or \
                     toxin against {prey_in:.3} gained from prey."
                ));
            }
            if reported {
                if ph.mouths == 0 && ph.jaw == 0.0 && ph.stinger == 0.0 {
                    notes.push(format!("{tag} has no mouth, so it can never eat: it can only starve. Add {{\"part\": \"mouth\"}}."));
                }
                if ph.thrust == 0.0 {
                    notes.push(format!(
                        "{tag} has no flagellum (or zero power), so it cannot move: it only eats what is in the cell it sits on."
                    ));
                }
                if ph.sensors.is_empty() {
                    notes.push(format!("{tag} has no chemo_sensor, so its brain gets no information about where food is."));
                }
            }
            if now_s.alive == 0 {
                if alive_before > 0 || released > 0 {
                    notes.push(format!(
                        "{tag} went extinct: {starved} starved and {killed} were killed by other players' organisms since the last report."
                    ));
                }
                continue;
            }
            notes.push(format!(
                "{tag}: {} alive ({:+} since the last report). {born} born, {starved} starved{}.",
                now_s.alive,
                now_s.alive as i64 - alive_before,
                age_at_death.map(|a| format!(", dying at {a:.0} ticks old on average")).unwrap_or_default()
            ));
            if ot > 0 {
                notes.push(format!(
                    "{tag} energy per organism per tick: {e_in:.4} eaten, {upkeep:.4} upkeep, {movement:.4} moving, net {net:+.4}."
                ));
            }
            if let (Some(e), Some(m)) = (mean_energy, mean_material) {
                if e >= split * 0.9 && m < 1.0 {
                    notes.push(format!(
                        "{tag} is mineral-limited: organisms average {:.0}% energy (division needs {:.0}%) \
                         but only {:.0}% of the mineral a child needs. More mineral intake, or a smaller body \
                         (child_material = 0.1*r^2), would speed division.",
                        e * 100.0, split * 100.0, m * 100.0
                    ));
                } else if e < 0.3 {
                    notes.push(format!(
                        "{tag} is energy-starved: organisms average {:.0}% of capacity. Cheaper upkeep, \
                         better food-finding (sensors) or more mouths would help.",
                        e * 100.0
                    ));
                }
            }
        }
        let room = self.world.cfg.max_population.saturating_sub(self.world.orgs.len());
        if room < self.world.cfg.max_population / 10 {
            notes.push(format!("The pool is nearly full: room for {room} more organisms in total."));
        }
        if mine.is_empty() {
            notes.push("You have no lineages yet: submit_genome, then release.".into());
        }
        let pl = &mut self.players[p];
        pl.last_report = snapshot;
        pl.last_report_tick = now;
        let mine_alive: u64 = mine.iter().map(|&l| self.world.lineages[l as usize].alive as u64).sum();
        let subs: Vec<Value> = self.players[p]
            .subs
            .iter()
            .map(|s| json!({"id": s.id, "event": s.event, "lineage": s.lineage, "value": s.value}))
            .collect();
        self.ok(json!({
            "since_tick": since,
            "world": {
                "population": self.world.orgs.len(),
                "yours": mine_alive,
                "others": self.world.orgs.len() as u64 - mine_alive,
                "sugar_total": self.world.grid.total(0),
                "mineral_total": self.world.grid.total(1),
                "deaths_cause": "every death is starvation (energy reached 0)",
            },
            "lineages": lineages,
            "subscriptions": subs,
            "notes": notes,
        }))
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
    /// A spectator tracking one organism: its state and next decision as
    /// JSON, or None if it is gone.
    Inspect { id: u64, reply: mpsc::Sender<Option<String>> },
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
    // Arena end_turn replies wait here until their window closes.
    let mut waiting: Vec<(usize, mpsc::Sender<(String, Option<usize>)>)> = Vec::new();
    loop {
        let msg = match game.arena_deadline() {
            Some(d) => match rx.recv_timeout(d.saturating_duration_since(std::time::Instant::now())) {
                Ok(m) => Some(m),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        if let Some(msg) = msg {
            handle_msg(&mut game, msg, &mut waiting);
        }
        for (p, reply) in game.arena_poll(std::time::Instant::now()) {
            waiting.retain(|(wp, tx)| {
                if *wp == p {
                    let _ = tx.send((reply.clone(), Some(p)));
                    false
                } else {
                    true
                }
            });
        }
    }
}

fn handle_msg(game: &mut Game, msg: Msg, waiting: &mut Vec<(usize, mpsc::Sender<(String, Option<usize>)>)>) {
    {
        match msg {
            Msg::Line { player, line, reply } => match game.handle_deferrable(player, &line) {
                (Some(out), p) => {
                    let _ = reply.send((out, p));
                }
                (None, p) => waiting.push((p.expect("deferred replies have a player"), reply)),
            },
            Msg::Snapshot { since_version, history_after, reply } => {
                let snap = if since_version == Some(game.version()) {
                    None
                } else {
                    Some(snapshot::build(&game, history_after))
                };
                let _ = reply.send(snap);
            }
            Msg::Inspect { id, reply } => {
                let _ = reply.send(snapshot::inspect(&game, id));
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
        // FOUNDER is radius 6: 5 x (6/5)^2 = 7.2 each. 69 of them = 496.8 of 500.
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"release","species":{sp},"count":69}}"#));
        assert!((r["budget"].as_f64().unwrap() - 3.2).abs() < 1e-6, "{r}");
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"release","species":{sp},"count":1}}"#));
        assert!(r["error"].as_str().unwrap().contains("budget"), "{r}");
        // A radius-20 body costs 80 per founder: no cheap biomass.
        assert!((founder_cost(20.0) - 80.0).abs() < 1e-9 && (founder_cost(5.0) - 5.0).abs() < 1e-9);
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

    fn founder_lineage(g: &mut Game) -> (Option<usize>, u64) {
        let (_, a) = call(g, None, r#"{"op":"hello","name":"a"}"#);
        let (r, _) = call(g, a, &format!(r#"{{"op":"submit_genome","genome":{}}}"#, scenario::FOUNDER));
        let sp = r["species"].as_u64().unwrap();
        let (r, _) = call(g, a, &format!(r#"{{"op":"release","species":{sp},"count":40}}"#));
        (a, r["lineage"].as_u64().unwrap())
    }

    #[test]
    fn report_gives_numbers_notes_and_resets() {
        let mut g = game();
        let (a, l) = founder_lineage(&mut g);
        call(&mut g, a, r#"{"op":"step","ticks":400}"#);
        let (r, _) = call(&mut g, a, r#"{"op":"report"}"#);
        assert_eq!(r["since_tick"], 0);
        let ln = &r["lineages"][0];
        assert_eq!(ln["lineage"], l);
        assert_eq!(ln["released"], 40);
        let alive = ln["alive"].as_u64().unwrap();
        assert_eq!(alive, 40 + ln["born"].as_u64().unwrap() - ln["starved"].as_u64().unwrap());
        assert!(ln["per_organism_per_tick"]["energy_in"].as_f64().unwrap() > 0.0, "{r}");
        let notes: Vec<&str> = r["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
        assert!(notes.iter().any(|n| n.contains("energy per organism per tick")), "{notes:?}");
        // A second report covers only what happened since the first.
        call(&mut g, a, r#"{"op":"step","ticks":100}"#);
        let (r2, _) = call(&mut g, a, r#"{"op":"report"}"#);
        assert_eq!(r2["since_tick"], 400);
        assert_eq!(r2["lineages"][0]["released"], 0);
        assert_eq!(r2["lineages"][0]["alive_at_last_report"], alive);
    }

    #[test]
    fn subscriptions_stop_a_step_early_once() {
        let mut g = game();
        let (a, l) = founder_lineage(&mut g);
        let (r, _) = call(&mut g, a, r#"{"op":"subscribe","event":"tick","value":250}"#);
        let tick_sub = r["id"].as_u64().unwrap();
        let (r, _) = call(&mut g, a, r#"{"op":"step","ticks":1000}"#);
        assert_eq!(r["tick"], 250, "{r}");
        assert_eq!(r["stepped"], 250);
        assert_eq!(r["woke"][0]["id"], tick_sub);
        // One-shot: the next step runs its full length.
        let (r, _) = call(&mut g, a, r#"{"op":"step","ticks":100}"#);
        assert_eq!(r["stepped"], 100);
        assert_eq!(r["woke"].as_array().unwrap().len(), 0);
        // population_above on a growing lineage fires; on someone else's is refused.
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"subscribe","event":"population_above","lineage":{l},"value":1000000}}"#));
        let never = r["id"].as_u64().unwrap();
        assert_eq!(r["true_now"], false);
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"unsubscribe","id":{never}}}"#));
        assert_eq!(r["removed"], never);
        let (_, b) = call(&mut g, None, r#"{"op":"hello","name":"b"}"#);
        let (r, _) = call(&mut g, b, &format!(r#"{{"op":"subscribe","event":"extinct","lineage":{l}}}"#));
        assert!(r["error"].as_str().unwrap().contains("not yours"), "{r}");
        let (r, _) = call(&mut g, a, r#"{"op":"subscribe","event":"tick","value":5}"#);
        assert!(r["error"].as_str().unwrap().contains("not in the future"), "{r}");
    }

    #[test]
    fn extinction_wakes_the_player() {
        // A body that can't eat starves: no mouth.
        let mut g = game();
        let (_, a) = call(&mut g, None, r#"{"op":"hello","name":"a"}"#);
        let mouthless = r#"{"schema":"lifeapi.genome/1","name":"Mouthless","body":[{"part":"membrane","radius":6},{"part":"flagellum","power":1.0}],"brain":{"hidden":2},"mutation":{"rate":0,"size":0}}"#;
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"submit_genome","genome":{mouthless}}}"#));
        let sp = r["species"].as_u64().unwrap();
        let (r, _) = call(&mut g, a, &format!(r#"{{"op":"release","species":{sp},"count":10}}"#));
        let l = r["lineage"].as_u64().unwrap();
        call(&mut g, a, &format!(r#"{{"op":"subscribe","event":"extinct","lineage":{l}}}"#));
        let (r, _) = call(&mut g, a, r#"{"op":"step","ticks":10000}"#);
        assert!(r["stepped"].as_u64().unwrap() < 10000, "{r}");
        assert_eq!(r["woke"][0]["event"], "extinct");
        let (r, _) = call(&mut g, a, r#"{"op":"report"}"#);
        let notes = r["notes"].to_string();
        assert!(notes.contains("went extinct"), "{notes}");
        // The extinct lineage still has its name and body, and the notes say why.
        assert_eq!(r["lineages"][0]["genome_name"], "Mouthless");
        assert_eq!(r["lineages"][0]["body"]["mouths"], 0);
        assert!(notes.contains("has no mouth"), "{notes}");
        assert!(notes.contains("no chemo_sensor"), "{notes}");
    }

    #[test]
    fn non_loopback_bind_is_refused() {
        // 0.0.0.0 binds all interfaces, which is not loopback.
        let e = bind("0.0.0.0:0").unwrap_err();
        assert!(e.to_string().contains("not loopback"), "{e}");
    }
}

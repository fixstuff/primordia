//! The world: grid + species + organisms, advanced one tick at a time.
//! Single-threaded and order-fixed: organisms are processed in Vec order,
//! newborns are appended, the dead are removed with an order-keeping retain.

use crate::brain;
use crate::det;
use crate::genome::{Chemical, Genome, Part, Phenotype, MAX_RADIUS, MAX_SENSORS, MIN_RADIUS};
use crate::grid::Grid;
use crate::rng::Pcg32;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct Vent {
    pub x: f32,
    pub y: f32,
    pub chem: usize,
    /// Added to the vent's cell every tick (the mean, when it pulses).
    pub rate: f32,
    /// Direction of drift, and offset of the pulse cycle. Set from the
    /// vent's index, never from the RNG, so static worlds draw nothing.
    pub heading: f32,
    pub phase: f32,
}

/// How the food supply changes over time. All zero = a static world
/// (no RNG draws, no change from the original sandbox).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dynamics {
    /// World units a vent moves per tick (it wanders: small random turns).
    pub drift: f32,
    /// Ticks per boom-and-bust cycle; 0 = no pulse.
    pub pulse_period: u32,
    /// 0..=1: the rate swings between (1-depth) and (1+depth) times the mean.
    pub pulse_depth: f32,
    /// Chance per vent per tick that it dies and reappears elsewhere.
    pub relocate_chance: f32,
    /// Sugar added to every cell every tick ("sunlight").
    pub background_sugar: f32,
}

impl Dynamics {
    pub fn is_static(&self) -> bool {
        *self == Dynamics::default()
    }
}

/// Who may fight whom. A match setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CombatMode {
    /// No combat: bites and stings do nothing (the server refuses combat parts).
    Off,
    /// Colony (like ants): organisms of the same team never fight or smell
    /// each other. The server gives each player one team.
    #[default]
    Colony,
    /// Every organism for itself, except family: no one fights its parent,
    /// its children or its siblings (same parent). Cousins and strangers,
    /// your own lineage included, are fair game. Everyone smells everyone
    /// but itself.
    Individual,
}

impl CombatMode {
    pub fn parse(s: &str) -> Result<CombatMode, String> {
        match s {
            "off" => Ok(CombatMode::Off),
            "colony" | "teams" => Ok(CombatMode::Colony),
            "individual" => Ok(CombatMode::Individual),
            _ => Err(format!("combat must be off, colony or individual, got {s}")),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            CombatMode::Off => "off",
            CombatMode::Colony => "colony",
            CombatMode::Individual => "individual",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub seed: u64,
    pub grid_w: usize,
    pub grid_h: usize,
    pub cell: f32,
    pub diffusion: f32,
    pub decay: f32,
    pub vents: Vec<Vent>,
    pub max_population: usize,
    /// Benchmark mode: no births, no deaths, energy floored at 0.
    pub fixed_population: bool,
    /// Per mouth per tick, in chemical units.
    pub eat_rate: f32,
    pub sugar_energy: f32,
    /// Energy per world unit moved.
    pub move_cost: f32,
    /// World units per tick at full thrust for a radius-6 body.
    pub max_speed: f32,
    /// Split when energy >= this fraction of capacity (and material allows).
    pub split_frac: f32,
    pub material_cap: f32,
    pub dynamics: Dynamics,
    pub combat: CombatMode,
    /// Chance per birth that the child's BODY mutates (0 = bodies never
    /// change; brains always mutate per the genome's own rate). A changed
    /// body is a new species in the same lineage, on the same team.
    pub body_mutation: f32,
}

#[derive(Clone, Debug)]
pub struct Organism {
    pub id: u64,
    pub lineage: u32,
    pub species: u32,
    pub x: f32,
    pub y: f32,
    pub heading: f32,
    pub energy: f32,
    pub material: f32,
    pub age: u32,
    /// 0 for a released founder, parent + 1 for a child. An observation
    /// only (nothing in the sim reads it), so it is left out of the hash.
    /// Arenas score only generation >= 1: founders are seeds, and a body
    /// that never reproduces scores nothing.
    pub generation: u32,
    /// The organism that divided to make this one; 0 for a released founder.
    /// Individual combat spares parent, children and siblings. Hashed only
    /// in individual mode (the only mode where it changes an outcome), so
    /// older hashes stand.
    pub parent: u64,
    pub brain: Vec<f32>,
    /// This tick's fight, for the viewer (observations, outside the hash).
    pub fight: Fight,
}

/// What happened to one organism in combat this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fight {
    /// The organism it bit or stung (0 = none) and the energy it took.
    pub target: u64,
    pub dealt: f32,
    /// The organism that bit or stung it (0 = none), its lineage, and the
    /// energy it lost.
    pub attacker: u64,
    pub attacker_lineage: u32,
    pub taken: f32,
    /// Energy lost to spikes or toxin this tick.
    pub hurt: f32,
}

#[derive(Clone, Debug)]
pub struct Species {
    pub genome: Genome,
    pub phen: Phenotype,
    /// Organisms of the same team never fight or smell each other. The
    /// server sets one team per player; 0 (the default) is one shared team,
    /// so a world nobody assigns teams in has no combat at all.
    pub team: u32,
    /// How an evolved body came to be ("urchin: + spikes"); empty for a
    /// submitted genome.
    pub origin: String,
}

/// One organism's state and its next decision (see `World::inspect`).
#[derive(Clone, Debug)]
pub struct Inspect {
    pub id: u64,
    pub lineage: u32,
    pub species: u32,
    pub name: String,
    pub generation: u32,
    pub age: u32,
    pub x: f32,
    pub y: f32,
    pub heading: f32,
    pub energy: f32,
    pub capacity: f32,
    pub material: f32,
    pub child_material: f32,
    pub split_energy: f32,
    pub upkeep: f32,
    pub mouths: u32,
    pub sugar_here: f32,
    pub mineral_here: f32,
    /// (angle off the heading in radians, chemical index, raw reading)
    pub sensors: Vec<(f32, usize, f32)>,
    /// Brain outputs: thrust (negative = stop) and turn, each -1..=1.
    pub thrust_out: f32,
    pub turn_out: f32,
    /// What those outputs do next tick: distance and radians.
    pub speed: f32,
    pub turn_rate: f32,
    /// Last tick's combat.
    pub fight: Fight,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub births: u64,
    pub deaths: u64,
}

/// Running totals for one lineage since it was released. These are
/// observations only: nothing in the simulation reads them, so they are
/// left out of `World::hash` (adding them changes no outcome).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineageStats {
    /// The species every member of this lineage belongs to.
    pub species: u32,
    pub alive: u32,
    pub released: u64,
    pub born: u64,
    /// Deaths with no attacker this tick: energy ran out.
    pub starved: u64,
    pub age_at_death_sum: u64,
    /// Organism-ticks lived (one per organism per tick).
    pub org_ticks: u64,
    /// Energy gained from sugar.
    pub energy_in: f64,
    pub upkeep: f64,
    pub movement: f64,
    pub mineral_in: f64,
    pub distance: f64,
    /// Combat: energy gained from biting/stinging, lost to attackers, lost
    /// to spikes and toxin, and members killed by another team.
    pub prey_in: f64,
    pub lost_to_attack: f64,
    pub hurt: f64,
    pub killed: u64,
    /// Enemy organisms this lineage's members killed (the victim died in a
    /// tick it was attacked by one of them).
    pub kills: u64,
}

pub struct World {
    pub tick: u64,
    pub cfg: Config,
    pub grid: Grid,
    pub rng: Pcg32,
    pub species: Vec<Species>,
    pub orgs: Vec<Organism>,
    pub stats: Stats,
    /// Indexed by lineage id (index 0 unused).
    pub lineages: Vec<LineageStats>,
    next_id: u64,
    next_lineage: u32,
    // Broadphase, reused every tick.
    nbx: usize,
    nby: usize,
    buckets: Vec<Vec<u32>>,
    /// Some species can bite or sting / can smell organisms: the combat and
    /// scent passes only run then, so worlds without them are untouched.
    combat: bool,
    smell: bool,
    /// Organism scent, by team, on a coarse grid (SCENT_CELL units a cell):
    /// body area of non-camouflaged organisms. scent[0] is every team.
    scent: Vec<Vec<f32>>,
    scent_w: usize,
    scent_h: usize,
}

/// Evolved species stop being created past this many species in a world.
pub const MAX_SPECIES: usize = 4096;
/// World units per scent cell.
const SCENT_CELL: f32 = 16.0;
/// Combat rates per tick of contact at power 1.
pub const JAW_RATE: f32 = 0.8;
pub const JAW_KEEP: f32 = 0.35;
pub const STING_RATE: f32 = 0.4;
pub const STING_KEEP: f32 = 0.2;
pub const SPIKE_RATE: f32 = 1.0;
/// How much of a jaw bite a full shell blocks.
pub const SHELL_BLOCK: f32 = 0.75;

impl World {
    pub fn new(cfg: Config) -> Result<World, String> {
        if !(0.0..=0.25).contains(&cfg.diffusion) {
            return Err("diffusion must be in 0..=0.25 (explicit scheme stability)".into());
        }
        let grid = Grid::new(cfg.grid_w, cfg.grid_h, cfg.cell);
        let bucket = 2.0 * MAX_RADIUS;
        let nbx = det::floor(grid.world_w() / bucket) as usize;
        let nby = det::floor(grid.world_h() / bucket) as usize;
        if nbx < 3 || nby < 3 {
            return Err(format!(
                "world must be at least {} units on each side (3 collision buckets)",
                3.0 * bucket
            ));
        }
        Ok(World {
            tick: 0,
            rng: Pcg32::new(cfg.seed, 0x5eed),
            grid,
            cfg,
            species: Vec::new(),
            orgs: Vec::new(),
            stats: Stats::default(),
            lineages: vec![LineageStats::default()],
            next_id: 1,
            next_lineage: 1,
            nbx,
            nby,
            buckets: vec![Vec::new(); nbx * nby],
            combat: false,
            smell: false,
            scent: Vec::new(),
            scent_w: 0,
            scent_h: 0,
        })
    }

    pub fn add_species(&mut self, genome: Genome) -> Result<u32, String> {
        let phen = genome.express()?;
        self.combat |= phen.jaw > 0.0 || phen.stinger > 0.0;
        self.smell |= phen.smells;
        self.species.push(Species { genome, phen, team: 0, origin: String::new() });
        Ok((self.species.len() - 1) as u32)
    }

    /// Release `n` founders of a species at random places as a new lineage.
    /// Founders with no template weights each get their own random brain.
    pub fn release(&mut self, species: u32, n: usize) -> Result<u32, String> {
        let sp = self
            .species
            .get(species as usize)
            .ok_or_else(|| format!("no species {species}"))?;
        let (phen, template) = (sp.phen.clone(), sp.genome.brain.weights.clone());
        let lineage = self.next_lineage;
        self.next_lineage += 1;
        let (ww, wh) = (self.grid.world_w(), self.grid.world_h());
        for _ in 0..n {
            let brain = if template.is_empty() {
                (0..phen.n_weights).map(|_| self.rng.range(-1.0, 1.0)).collect()
            } else {
                template.clone()
            };
            let o = Organism {
                id: self.next_id,
                lineage,
                species,
                x: self.rng.range(0.0, ww),
                y: self.rng.range(0.0, wh),
                heading: self.rng.range(-det::PI, det::PI),
                energy: phen.founder_energy,
                material: 0.0,
                age: 0,
                generation: 0,
                parent: 0,
                brain,
                fight: Fight::default(),
            };
            self.next_id += 1;
            self.orgs.push(o);
        }
        self.lineages.push(LineageStats { species, alive: n as u32, released: n as u64, ..Default::default() });
        debug_assert_eq!(self.lineages.len(), lineage as usize + 1);
        Ok(lineage)
    }

    pub fn tick(&mut self) {
        self.step_grid();
        if self.smell {
            self.step_scent();
        }
        self.step_organisms();
        self.step_collisions();
        if self.combat && self.cfg.combat != CombatMode::Off {
            self.step_combat();
        }
        if !self.cfg.fixed_population {
            self.step_deaths();
            self.step_births();
        }
        self.tick += 1;
    }

    pub fn step_grid(&mut self) {
        let World { cfg, grid, rng, tick, .. } = self;
        let d = &cfg.dynamics;
        let (ww, wh) = (grid.world_w(), grid.world_h());
        for v in cfg.vents.iter_mut() {
            let mut rate = v.rate;
            if d.pulse_period > 0 {
                let t = (*tick % d.pulse_period as u64) as f32 / d.pulse_period as f32;
                rate = det::max(rate * (1.0 + d.pulse_depth * det::sin(det::TAU * t + v.phase)), 0.0);
            }
            grid.add(v.x, v.y, v.chem, rate);
            if d.drift > 0.0 {
                v.heading = det::wrap_angle(v.heading + rng.range(-0.05, 0.05));
                v.x = det::wrap(v.x + det::cos(v.heading) * d.drift, ww);
                v.y = det::wrap(v.y + det::sin(v.heading) * d.drift, wh);
            }
            if d.relocate_chance > 0.0 && rng.next_f32() < d.relocate_chance {
                v.x = rng.range(0.0, ww);
                v.y = rng.range(0.0, wh);
            }
        }
        if d.background_sugar > 0.0 {
            for c in grid.chem[0].iter_mut() {
                *c += d.background_sugar;
            }
        }
        grid.diffuse(cfg.diffusion, cfg.decay);
    }

    pub fn step_organisms(&mut self) {
        let World { orgs, species, grid, cfg, lineages, scent, scent_w, scent_h, .. } = self;
        let (ww, wh) = (grid.world_w(), grid.world_h());
        let mut inputs = [0f32; MAX_SENSORS + 2];
        for o in orgs.iter_mut() {
            let ls = &mut lineages[o.lineage as usize];
            let p = &species[o.species as usize].phen;
            // Sense.
            let ns = p.sensors.len();
            let team = species[o.species as usize].team;
            let me = (cfg.combat == CombatMode::Individual).then_some((o.x, o.y, p.radius * p.radius, p.camouflage));
            for (k, &(ang, c)) in p.sensors.iter().enumerate() {
                let a = o.heading + ang;
                let reach = p.radius + 2.0;
                let (sx, sy) = (o.x + det::cos(a) * reach, o.y + det::sin(a) * reach);
                let v = if c == 2 {
                    scent_at(scent, (*scent_w, *scent_h, ww, wh), sx, sy, team, me)
                } else {
                    grid.sample(sx, sy, c)
                };
                inputs[k] = v / (v + 1.0);
            }
            inputs[ns] = o.energy / p.capacity;
            inputs[ns + 1] = 1.0;
            // Think.
            let out = brain::forward(&o.brain, &inputs[..ns + 2], p.hidden);
            // Act: turn, then move. Bigger bodies are slower.
            o.heading = det::wrap_angle(o.heading + out[1] * p.turn);
            let speed = cfg.max_speed * det::max(out[0], 0.0) * p.thrust * (6.0 / p.radius);
            o.x = det::wrap(o.x + det::cos(o.heading) * speed, ww);
            o.y = det::wrap(o.y + det::sin(o.heading) * speed, wh);
            // Eat.
            if p.mouths > 0 {
                let bite = cfg.eat_rate * p.mouths as f32;
                let room = det::max(p.capacity - o.energy, 0.0) / cfg.sugar_energy;
                let gained = grid.take(o.x, o.y, 0, det::min(bite, room)) * cfg.sugar_energy;
                o.energy += gained;
                let mroom = det::max(cfg.material_cap - o.material, 0.0);
                let mgained = grid.take(o.x, o.y, 1, det::min(bite, mroom));
                o.material += mgained;
                ls.energy_in += gained as f64;
                ls.mineral_in += mgained as f64;
            }
            // Pay.
            let move_cost = speed * cfg.move_cost;
            o.energy -= p.upkeep + move_cost;
            if cfg.fixed_population {
                o.energy = det::max(o.energy, 0.0);
            }
            o.age = o.age.saturating_add(1);
            ls.upkeep += p.upkeep as f64;
            ls.movement += move_cost as f64;
            ls.distance += speed as f64;
            ls.org_ticks += 1;
        }
    }

    /// What one organism senses and decides right now: the same sense and
    /// think steps `step_organisms` runs, recomputed read-only (nothing is
    /// changed, so the hash is untouched). For the viewer's "track" panel.
    pub fn inspect(&self, id: u64) -> Option<Inspect> {
        let o = self.orgs.iter().find(|o| o.id == id)?;
        let sp = &self.species[o.species as usize];
        let p = &sp.phen;
        let ns = p.sensors.len();
        let mut inputs = vec![0f32; ns + 2];
        let mut sensors = Vec::with_capacity(ns);
        for (k, &(ang, c)) in p.sensors.iter().enumerate() {
            let a = o.heading + ang;
            let reach = p.radius + 2.0;
            let (sx, sy) = (o.x + det::cos(a) * reach, o.y + det::sin(a) * reach);
            let me = (self.cfg.combat == CombatMode::Individual).then_some((o.x, o.y, p.radius * p.radius, p.camouflage));
            let v = if c == 2 {
                scent_at(&self.scent, (self.scent_w, self.scent_h, self.grid.world_w(), self.grid.world_h()), sx, sy, sp.team, me)
            } else {
                self.grid.sample(sx, sy, c)
            };
            inputs[k] = v / (v + 1.0);
            sensors.push((ang, c, v));
        }
        inputs[ns] = o.energy / p.capacity;
        inputs[ns + 1] = 1.0;
        let out = brain::forward(&o.brain, &inputs, p.hidden);
        let speed = self.cfg.max_speed * det::max(out[0], 0.0) * p.thrust * (6.0 / p.radius);
        Some(Inspect {
            id: o.id,
            lineage: o.lineage,
            species: o.species,
            name: sp.genome.name.clone(),
            generation: o.generation,
            age: o.age,
            x: o.x,
            y: o.y,
            heading: o.heading,
            energy: o.energy,
            capacity: p.capacity,
            material: o.material,
            child_material: p.child_material,
            split_energy: self.cfg.split_frac * p.capacity,
            upkeep: p.upkeep,
            mouths: p.mouths,
            sugar_here: self.grid.sample(o.x, o.y, 0),
            mineral_here: self.grid.sample(o.x, o.y, 1),
            sensors,
            thrust_out: out[0],
            turn_out: out[1],
            speed,
            turn_rate: out[1] * p.turn,
            fight: o.fight.clone(),
        })
    }

    /// Lay down this tick's organism scent: each non-camouflaged body adds its
    /// area to its coarse cell, for its team and for everyone (index 0).
    fn step_scent(&mut self) {
        let (ww, wh) = (self.grid.world_w(), self.grid.world_h());
        let sw = det::max(det::floor(ww / SCENT_CELL), 1.0) as usize;
        let sh = det::max(det::floor(wh / SCENT_CELL), 1.0) as usize;
        let teams = self.species.iter().map(|s| s.team as usize).fold(0, std::cmp::max) + 2;
        if self.scent.len() != teams || self.scent_w != sw || self.scent_h != sh {
            self.scent = vec![vec![0.0; sw * sh]; teams];
            self.scent_w = sw;
            self.scent_h = sh;
        }
        for layer in self.scent.iter_mut() {
            layer.iter_mut().for_each(|v| *v = 0.0);
        }
        for o in &self.orgs {
            let sp = &self.species[o.species as usize];
            if sp.phen.camouflage {
                continue;
            }
            let (cx, cy) = (scent_cell(o.x, ww, sw), scent_cell(o.y, wh, sh));
            let a = sp.phen.radius * sp.phen.radius;
            self.scent[0][cy * sw + cx] += a;
            self.scent[sp.team as usize + 1][cy * sw + cx] += a;
        }
    }

    /// Bites and stings between touching organisms of different teams.
    /// Effects are summed for the tick and applied together, so the order
    /// pairs are visited in never decides who wins.
    fn step_combat(&mut self) {
        let (ww, wh) = (self.grid.world_w(), self.grid.world_h());
        let n = self.orgs.len();
        let mut de = vec![0f32; n];
        let mut dm = vec![0f32; n];
        for o in self.orgs.iter_mut() {
            o.fight = Fight::default();
        }
        let (nbx, nby) = (self.nbx, self.nby);
        let mode = self.cfg.combat;
        let World { orgs, species, buckets, lineages, cfg, .. } = self;
        // Buckets hold indices from this tick's collision pass; bodies have
        // only been nudged since, so neighbours are still neighbours.
        for by in 0..nby {
            for bx in 0..nbx {
                for &i in &buckets[by * nbx + bx] {
                    for dy in [nby - 1, 0, 1] {
                        for dx in [nbx - 1, 0, 1] {
                            let nb = ((by + dy) % nby) * nbx + (bx + dx) % nbx;
                            for &j in &buckets[nb] {
                                if j <= i {
                                    continue;
                                }
                                let (i, j) = (i as usize, j as usize);
                                let (si, sj) = (&species[orgs[i].species as usize], &species[orgs[j].species as usize]);
                                if mode == CombatMode::Colony && si.team == sj.team {
                                    continue;
                                }
                                if mode == CombatMode::Individual && family(&orgs[i], &orgs[j]) {
                                    continue;
                                }
                                let rr = si.phen.radius + sj.phen.radius;
                                let ddx = det::torus_delta(orgs[i].x, orgs[j].x, ww);
                                let ddy = det::torus_delta(orgs[i].y, orgs[j].y, wh);
                                // Touching: within a 10% margin of overlap, since
                                // the collision pass has just pushed them apart.
                                if ddx * ddx + ddy * ddy > (1.1 * rr) * (1.1 * rr) {
                                    continue;
                                }
                                attack(orgs, &mut de, &mut dm, species, lineages, i, j);
                                attack(orgs, &mut de, &mut dm, species, lineages, j, i);
                            }
                        }
                    }
                }
            }
        }
        for (k, o) in orgs.iter_mut().enumerate() {
            let p = &species[o.species as usize].phen;
            o.energy = det::min(o.energy + de[k], p.capacity);
            o.material = det::max(det::min(o.material + dm[k], cfg.material_cap), 0.0);
        }
    }

    /// Soft push-apart of overlapping bodies, grid broadphase on the torus.
    pub fn step_collisions(&mut self) {
        let (ww, wh) = (self.grid.world_w(), self.grid.world_h());
        let (nbx, nby) = (self.nbx, self.nby);
        let (bw, bh) = (ww / nbx as f32, wh / nby as f32);
        for b in self.buckets.iter_mut() {
            b.clear();
        }
        for (i, o) in self.orgs.iter().enumerate() {
            let bx = det::floor(o.x / bw) as usize % nbx;
            let by = det::floor(o.y / bh) as usize % nby;
            self.buckets[by * nbx + bx].push(i as u32);
        }
        let World { orgs, species, buckets, .. } = self;
        for by in 0..nby {
            for bx in 0..nbx {
                let here = &buckets[by * nbx + bx];
                for &i in here {
                    for dy in [nby - 1, 0, 1] {
                        for dx in [nbx - 1, 0, 1] {
                            let nb = ((by + dy) % nby) * nbx + (bx + dx) % nbx;
                            for &j in &buckets[nb] {
                                if j <= i {
                                    continue;
                                }
                                let (i, j) = (i as usize, j as usize);
                                let ri = species[orgs[i].species as usize].phen.radius;
                                let rj = species[orgs[j].species as usize].phen.radius;
                                let ddx = det::torus_delta(orgs[i].x, orgs[j].x, ww);
                                let ddy = det::torus_delta(orgs[i].y, orgs[j].y, wh);
                                let d2 = ddx * ddx + ddy * ddy;
                                let rr = ri + rj;
                                if d2 >= rr * rr || d2 < 1e-12 {
                                    continue;
                                }
                                let d = d2.sqrt();
                                let push = 0.25 * (rr - d) / d;
                                let (px, py) = (ddx * push, ddy * push);
                                orgs[i].x = det::wrap(orgs[i].x - px, ww);
                                orgs[i].y = det::wrap(orgs[i].y - py, wh);
                                orgs[j].x = det::wrap(orgs[j].x + px, ww);
                                orgs[j].y = det::wrap(orgs[j].y + py, wh);
                            }
                        }
                    }
                }
            }
        }
    }

    /// The dead return their body to the pool where they fell.
    pub fn step_deaths(&mut self) {
        let World { orgs, species, grid, stats, lineages, .. } = self;
        for o in orgs.iter() {
            if o.energy <= 0.0 {
                let p = &species[o.species as usize].phen;
                grid.add(o.x, o.y, 1, o.material + p.child_material);
                grid.add(o.x, o.y, 0, 0.02 * p.radius * p.radius);
                stats.deaths += 1;
                let ls = &mut lineages[o.lineage as usize];
                ls.alive -= 1;
                if o.fight.attacker != 0 || o.fight.hurt > 0.0 {
                    ls.killed += 1;
                } else {
                    ls.starved += 1;
                }
                ls.age_at_death_sum += o.age as u64;
                if o.fight.attacker != 0 {
                    lineages[o.fight.attacker_lineage as usize].kills += 1;
                }
            }
        }
        orgs.retain(|o| o.energy > 0.0);
    }

    /// Division: half the energy goes to the child, plus one body's worth of
    /// material. The child's brain mutates.
    pub fn step_births(&mut self) {
        let (ww, wh) = (self.grid.world_w(), self.grid.world_h());
        let n = self.orgs.len();
        for i in 0..n {
            if self.orgs.len() >= self.cfg.max_population {
                break;
            }
            let sp = &self.species[self.orgs[i].species as usize];
            let (cap, need, r) = (sp.phen.capacity, sp.phen.child_material, sp.phen.radius);
            let (rate, size) = (sp.genome.mutation.rate, sp.genome.mutation.size);
            let o = &mut self.orgs[i];
            if o.energy < self.cfg.split_frac * cap || o.material < need {
                continue;
            }
            o.energy *= 0.5;
            o.material -= need;
            let mut child = o.clone();
            child.id = self.next_id;
            self.next_id += 1;
            child.age = 0;
            child.generation = o.generation.saturating_add(1);
            child.fight = Fight::default();
            child.parent = o.id;
            child.material = 0.0;
            child.heading = det::wrap_angle(o.heading + det::PI);
            child.x = det::wrap(o.x + det::cos(child.heading) * r, ww);
            child.y = det::wrap(o.y + det::sin(child.heading) * r, wh);
            for w in child.brain.iter_mut() {
                if self.rng.next_f32() < rate {
                    *w = det::clamp(*w + self.rng.range(-size, size), -4.0, 4.0);
                }
            }
            // Body evolution: the RNG is only drawn when it is on, so worlds
            // without it replay exactly as before.
            if self.cfg.body_mutation > 0.0 && self.rng.next_f32() < self.cfg.body_mutation {
                self.evolve_body(&mut child);
            }
            let ls = &mut self.lineages[child.lineage as usize];
            ls.alive += 1;
            ls.born += 1;
            self.orgs.push(child);
            self.stats.births += 1;
        }
    }

    /// Mutate a newborn's body: nudge one part, add a part, or drop one. The
    /// brain is reshaped to match (a new sensor gets zero weights; a lost
    /// sensor's weights go). An identical body already on this team is
    /// reused, so species don't multiply without need. An invalid result
    /// (too many parts or sensors) leaves the child as it was.
    fn evolve_body(&mut self, child: &mut Organism) {
        let parent = &self.species[child.species as usize];
        let (team, mut g) = (parent.team, parent.genome.clone());
        let combat = self.cfg.combat != CombatMode::Off;
        let sensor_index = |body: &[Part], i: usize| body[..i].iter().filter(|p| matches!(p, Part::ChemoSensor { .. })).count();
        let n = g.body.len();
        // None: no brain change. Some((true, k)): insert input k. Some((false, k)): remove input k.
        let mut brain_edit: Option<(bool, usize)> = None;
        let what: String;
        let part_name = |p: &Part| serde_json::to_value(p).ok().and_then(|v| v["part"].as_str().map(String::from)).unwrap_or_default();
        match self.rng.below(3) {
            0 => {
                let i = self.rng.below(n as u32) as usize;
                what = format!("~ {}", part_name(&g.body[i]));
                let rng = &mut self.rng;
                let mut nudge = |v: f32, d: f32, lo: f32, hi: f32| det::clamp(v + rng.range(-d, d), lo, hi);
                g.body[i] = match g.body[i].clone() {
                    Part::Membrane { radius } => Part::Membrane { radius: nudge(radius, 0.15 * radius, MIN_RADIUS, MAX_RADIUS) },
                    Part::Flagellum { power } => Part::Flagellum { power: nudge(power, 0.2, 0.0, 1.0) },
                    Part::Cilia { power } => Part::Cilia { power: nudge(power, 0.2, 0.0, 1.0) },
                    Part::ChemoSensor { angle, chemical } => Part::ChemoSensor { angle: nudge(angle, 20.0, -180.0, 180.0), chemical },
                    Part::Vacuole { capacity } => Part::Vacuole { capacity: nudge(capacity, 20.0, 0.0, 100.0) },
                    Part::Jaw { power } => Part::Jaw { power: nudge(power, 0.2, 0.0, 1.0) },
                    Part::Stinger { power } => Part::Stinger { power: nudge(power, 0.2, 0.0, 1.0) },
                    Part::Shell { thickness } => Part::Shell { thickness: nudge(thickness, 0.2, 0.0, 1.0) },
                    Part::Spikes { power } => Part::Spikes { power: nudge(power, 0.2, 0.0, 1.0) },
                    Part::Toxin { power } => Part::Toxin { power: nudge(power, 0.2, 0.0, 1.0) },
                    p @ (Part::Mouth | Part::Camouflage) => p,
                };
            }
            1 => {
                let kinds = if combat { 11 } else { 5 };
                let chems = if combat { 3 } else { 2 };
                let part = match self.rng.below(kinds) {
                    0 => Part::Flagellum { power: 0.3 },
                    1 => Part::Cilia { power: 0.3 },
                    2 => Part::Mouth,
                    3 => Part::ChemoSensor {
                        angle: self.rng.range(-90.0, 90.0),
                        chemical: [Chemical::Sugar, Chemical::Mineral, Chemical::Organism][self.rng.below(chems) as usize],
                    },
                    4 => Part::Vacuole { capacity: 20.0 },
                    5 => Part::Jaw { power: 0.3 },
                    6 => Part::Stinger { power: 0.3 },
                    7 => Part::Shell { thickness: 0.3 },
                    8 => Part::Spikes { power: 0.3 },
                    9 => Part::Camouflage,
                    _ => Part::Toxin { power: 0.3 },
                };
                if matches!(part, Part::ChemoSensor { .. }) {
                    brain_edit = Some((true, sensor_index(&g.body, n)));
                }
                what = format!("+ {}", part_name(&part));
                g.body.push(part);
            }
            _ => {
                let i = self.rng.below(n as u32) as usize;
                if matches!(g.body[i], Part::Membrane { .. }) {
                    return;
                }
                if matches!(g.body[i], Part::ChemoSensor { .. }) {
                    brain_edit = Some((false, sensor_index(&g.body, i)));
                }
                what = format!("- {}", part_name(&g.body[i]));
                g.body.remove(i);
            }
        }
        g.brain.weights.clear();
        let Ok(phen) = g.express() else { return };
        if g.body == parent.genome.body {
            return;
        }
        // Reshape the brain: hidden rows are [hidden][inputs]; inputs are
        // the sensors in body order, then energy, then the bias.
        let old_in = phen.n_in as i64 + match brain_edit {
            Some((true, _)) => -1,
            Some((false, _)) => 1,
            None => 0,
        };
        if let Some((insert, k)) = brain_edit {
            let old_in = old_in as usize;
            let mut w = Vec::with_capacity(phen.n_weights);
            for row in 0..phen.hidden {
                let r = &child.brain[row * old_in..(row + 1) * old_in];
                for (c, v) in r.iter().enumerate() {
                    if insert && c == k {
                        w.push(0.0);
                    }
                    if !insert && c == k {
                        continue;
                    }
                    w.push(*v);
                }
                if insert && k == old_in {
                    w.push(0.0);
                }
            }
            w.extend_from_slice(&child.brain[phen.hidden * old_in..]);
            debug_assert_eq!(w.len(), phen.n_weights);
            child.brain = w;
        }
        let existing = self
            .species
            .iter()
            .position(|s| s.team == team && s.genome.body == g.body && s.genome.brain.hidden == g.brain.hidden);
        child.species = match existing {
            Some(s) => s as u32,
            None if self.species.len() < MAX_SPECIES => {
                let base = g.name.split('~').next().unwrap_or("evolved").to_string();
                let origin = format!("{}: {what}", parent.genome.name);
                g.name = format!("{base}~{}", self.species.len());
                self.combat |= phen.jaw > 0.0 || phen.stinger > 0.0;
                self.smell |= phen.smells;
                self.species.push(Species { genome: g, phen, team, origin });
                (self.species.len() - 1) as u32
            }
            None => return,
        };
    }

    /// SHA-256 over the entire state, in a fixed order, as raw bits.
    pub fn hash(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.tick.to_le_bytes());
        h.update(self.rng.state_bytes());
        h.update(self.next_id.to_le_bytes());
        h.update(self.next_lineage.to_le_bytes());
        for c in &self.grid.chem {
            for v in c {
                h.update(v.to_bits().to_le_bytes());
            }
        }
        h.update((self.orgs.len() as u64).to_le_bytes());
        for o in &self.orgs {
            h.update(o.id.to_le_bytes());
            h.update(o.lineage.to_le_bytes());
            h.update(o.species.to_le_bytes());
            for f in [o.x, o.y, o.heading, o.energy, o.material] {
                h.update(f.to_bits().to_le_bytes());
            }
            h.update(o.age.to_le_bytes());
            if self.cfg.combat == CombatMode::Individual {
                h.update(o.parent.to_le_bytes());
            }
            for w in &o.brain {
                h.update(w.to_bits().to_le_bytes());
            }
        }
        h.update(self.stats.births.to_le_bytes());
        h.update(self.stats.deaths.to_le_bytes());
        // Moving vents are state. Static worlds leave them out, so the
        // original golden hash is unchanged.
        if !self.cfg.dynamics.is_static() {
            for v in &self.cfg.vents {
                for f in [v.x, v.y, v.heading] {
                    h.update(f.to_bits().to_le_bytes());
                }
            }
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Scent around (x, y) over the 3x3 coarse cells, scaled so one radius-5
/// body reads about 1. Colony mode: everyone minus your own team. Individual
/// mode (`me` = your x, y, area, camouflage): everyone minus your own body.
fn scent_at(
    scent: &[Vec<f32>],
    (sw, sh, ww, wh): (usize, usize, f32, f32),
    x: f32,
    y: f32,
    team: u32,
    me: Option<(f32, f32, f32, bool)>,
) -> f32 {
    if scent.is_empty() || sw == 0 || sh == 0 {
        return 0.0;
    }
    let cx = scent_cell(x, ww, sw) as i64;
    let cy = scent_cell(y, wh, sh) as i64;
    let own = &scent[team as usize + 1];
    let my_cell = me.map(|(mx, my, _, _)| (scent_cell(mx, ww, sw), scent_cell(my, wh, sh)));
    let mut v = 0.0;
    for dy in -1..=1i64 {
        for dx in -1..=1i64 {
            let gx = (cx + dx).rem_euclid(sw as i64) as usize;
            let gy = (cy + dy).rem_euclid(sh as i64) as usize;
            match me {
                None => v += scent[0][gy * sw + gx] - own[gy * sw + gx],
                Some((_, _, area, camo)) => {
                    v += scent[0][gy * sw + gx];
                    if !camo && my_cell == Some((gx, gy)) {
                        v -= area;
                    }
                }
            }
        }
    }
    det::max(v, 0.0) / 25.0
}

/// Parent and child, or siblings (same parent). Founders have no parent.
fn family(a: &Organism, b: &Organism) -> bool {
    a.parent == b.id || b.parent == a.id || (a.parent != 0 && a.parent == b.parent)
}

/// The coarse scent cell holding coordinate `v` on an axis of `size` world
/// units split into `n` cells. Deposits and samples both use this.
fn scent_cell(v: f32, size: f32, n: usize) -> usize {
    std::cmp::min(det::floor(det::wrap(v, size) / size * n as f32) as usize, n - 1)
}

/// Organism `a` attacks `b` (different teams, touching) with whatever
/// weapons it has. Energy taken is limited by what `b` had this tick.
fn attack(
    orgs: &mut [Organism],
    de: &mut [f32],
    dm: &mut [f32],
    species: &[Species],
    lineages: &mut [LineageStats],
    a: usize,
    b: usize,
) {
    let (pa, pb) = (&species[orgs[a].species as usize].phen, &species[orgs[b].species as usize].phen);
    if pa.jaw <= 0.0 && pa.stinger <= 0.0 {
        return;
    }
    let mut taken = 0.0f32;
    let mut gained = 0.0f32;
    let left = |de_b: f32| det::max(orgs[b].energy + de_b, 0.0);
    if pa.jaw > 0.0 && pb.radius < pa.radius {
        let bite = JAW_RATE * pa.jaw * (1.0 - SHELL_BLOCK * pb.shell);
        let t = det::min(bite, left(de[b]));
        de[b] -= t;
        taken += t;
        gained += t * JAW_KEEP * (1.0 - 2.0 * pb.toxin);
        let m = det::min(0.5 * bite, det::max(orgs[b].material + dm[b], 0.0));
        dm[b] -= m;
        dm[a] += 0.5 * m;
    }
    if pa.stinger > 0.0 {
        let t = det::min(STING_RATE * pa.stinger, left(de[b]));
        de[b] -= t;
        taken += t;
        gained += t * STING_KEEP * (1.0 - 2.0 * pb.toxin);
    }
    if taken <= 0.0 {
        return;
    }
    de[a] += gained;
    let spiked = SPIKE_RATE * pb.spikes;
    de[a] -= spiked;
    let (ida, idb) = (orgs[a].id, orgs[b].id);
    let (la, lb) = (orgs[a].lineage as usize, orgs[b].lineage as usize);
    orgs[a].fight.target = idb;
    orgs[a].fight.dealt += taken;
    orgs[a].fight.hurt += spiked + det::max(-gained, 0.0);
    orgs[b].fight.attacker = ida;
    orgs[b].fight.attacker_lineage = la as u32;
    orgs[b].fight.taken += taken;
    lineages[la].prey_in += det::max(gained, 0.0) as f64;
    lineages[la].hurt += (spiked + det::max(-gained, 0.0)) as f64;
    lineages[lb].lost_to_attack += taken as f64;
}

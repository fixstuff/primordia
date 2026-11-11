//! The world: grid + species + organisms, advanced one tick at a time.
//! Single-threaded and order-fixed: organisms are processed in Vec order,
//! newborns are appended, the dead are removed with an order-keeping retain.

use crate::brain;
use crate::det;
use crate::genome::{Genome, Phenotype, MAX_RADIUS, MAX_SENSORS};
use crate::grid::Grid;
use crate::rng::Pcg32;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct Vent {
    pub x: f32,
    pub y: f32,
    pub chem: usize,
    /// Added to the vent's cell every tick.
    pub rate: f32,
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
    pub brain: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Species {
    pub genome: Genome,
    pub phen: Phenotype,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub births: u64,
    pub deaths: u64,
}

pub struct World {
    pub tick: u64,
    pub cfg: Config,
    pub grid: Grid,
    pub rng: Pcg32,
    pub species: Vec<Species>,
    pub orgs: Vec<Organism>,
    pub stats: Stats,
    next_id: u64,
    next_lineage: u32,
    // Broadphase, reused every tick.
    nbx: usize,
    nby: usize,
    buckets: Vec<Vec<u32>>,
}

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
            next_id: 1,
            next_lineage: 1,
            nbx,
            nby,
            buckets: vec![Vec::new(); nbx * nby],
        })
    }

    pub fn add_species(&mut self, genome: Genome) -> Result<u32, String> {
        let phen = genome.express()?;
        self.species.push(Species { genome, phen });
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
                energy: 0.5 * phen.capacity,
                material: 0.0,
                age: 0,
                brain,
            };
            self.next_id += 1;
            self.orgs.push(o);
        }
        Ok(lineage)
    }

    pub fn tick(&mut self) {
        self.step_grid();
        self.step_organisms();
        self.step_collisions();
        if !self.cfg.fixed_population {
            self.step_deaths();
            self.step_births();
        }
        self.tick += 1;
    }

    pub fn step_grid(&mut self) {
        for v in &self.cfg.vents {
            self.grid.add(v.x, v.y, v.chem, v.rate);
        }
        self.grid.diffuse(self.cfg.diffusion, self.cfg.decay);
    }

    pub fn step_organisms(&mut self) {
        let World { orgs, species, grid, cfg, .. } = self;
        let (ww, wh) = (grid.world_w(), grid.world_h());
        let mut inputs = [0f32; MAX_SENSORS + 2];
        for o in orgs.iter_mut() {
            let p = &species[o.species as usize].phen;
            // Sense.
            let ns = p.sensors.len();
            for (k, &(ang, c)) in p.sensors.iter().enumerate() {
                let a = o.heading + ang;
                let reach = p.radius + 2.0;
                let v = grid.sample(o.x + det::cos(a) * reach, o.y + det::sin(a) * reach, c);
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
                o.energy += grid.take(o.x, o.y, 0, det::min(bite, room)) * cfg.sugar_energy;
                let mroom = det::max(cfg.material_cap - o.material, 0.0);
                o.material += grid.take(o.x, o.y, 1, det::min(bite, mroom));
            }
            // Pay.
            o.energy -= p.upkeep + speed * cfg.move_cost;
            if cfg.fixed_population {
                o.energy = det::max(o.energy, 0.0);
            }
            o.age = o.age.saturating_add(1);
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
        let World { orgs, species, grid, stats, .. } = self;
        for o in orgs.iter() {
            if o.energy <= 0.0 {
                let p = &species[o.species as usize].phen;
                grid.add(o.x, o.y, 1, o.material + p.child_material);
                grid.add(o.x, o.y, 0, 0.02 * p.radius * p.radius);
                stats.deaths += 1;
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
            child.material = 0.0;
            child.heading = det::wrap_angle(o.heading + det::PI);
            child.x = det::wrap(o.x + det::cos(child.heading) * r, ww);
            child.y = det::wrap(o.y + det::sin(child.heading) * r, wh);
            for w in child.brain.iter_mut() {
                if self.rng.next_f32() < rate {
                    *w = det::clamp(*w + self.rng.range(-size, size), -4.0, 4.0);
                }
            }
            self.orgs.push(child);
            self.stats.births += 1;
        }
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
            for w in &o.brain {
                h.update(w.to_bits().to_le_bytes());
            }
        }
        h.update(self.stats.births.to_le_bytes());
        h.update(self.stats.deaths.to_le_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

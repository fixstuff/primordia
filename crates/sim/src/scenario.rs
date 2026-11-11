//! Fixed scenarios. `golden` is the determinism reference and `bench` the
//! performance one: change either and the committed numbers mean nothing.

use crate::genome::Genome;
use crate::world::{Config, Vent, World};

/// The default founder: one flagellum, cilia, a mouth, two sugar sensors.
pub const FOUNDER: &str = r#"{
  "schema": "lifeapi.genome/1",
  "name": "Driftwhip",
  "body": [
    {"part": "membrane", "radius":6},
    {"part": "flagellum", "power": 0.6},
    {"part": "cilia", "power": 0.5},
    {"part": "mouth"},
    {"part": "chemo_sensor", "angle": -30, "chemical": "sugar"},
    {"part": "chemo_sensor", "angle": 30, "chemical": "sugar"}
  ],
  "brain": {"hidden":6},
  "mutation": {"rate": 0.05, "size": 0.3}
}"#;

pub fn base_config(seed: u64, grid_w: usize, grid_h: usize) -> Config {
    Config {
        seed,
        grid_w,
        grid_h,
        cell: 4.0,
        diffusion: 0.1,
        decay: 0.0005,
        vents: Vec::new(),
        max_population: 4000,
        fixed_population: false,
        eat_rate: 0.2,
        sugar_energy: 1.0,
        move_cost: 0.01,
        max_speed: 1.0,
        split_frac: 0.8,
        material_cap: 20.0,
    }
}

/// A pool with a seeded chemical field and vents, plus `founders` organisms
/// of the default genome.
pub fn sandbox(mut cfg: Config, sugar_vents: usize, mineral_vents: usize, founders: usize) -> World {
    let mut w = World::new(cfg.clone()).expect("scenario config is valid");
    let (ww, wh) = (w.grid.world_w(), w.grid.world_h());
    for k in 0..sugar_vents + mineral_vents {
        let (chem, rate) = if k < sugar_vents { (0, 3.0) } else { (1, 1.0) };
        cfg.vents.push(Vent { x: w.rng.range(0.0, ww), y: w.rng.range(0.0, wh), chem, rate });
    }
    w.cfg.vents = cfg.vents;
    for i in 0..w.grid.chem[0].len() {
        w.grid.chem[0][i] = 0.5 + 0.5 * w.rng.next_f32();
        w.grid.chem[1][i] = 0.3;
    }
    let sp = w.add_species(Genome::from_json(FOUNDER).expect("FOUNDER parses")).expect("FOUNDER expresses");
    w.release(sp, founders).expect("species exists");
    w
}

/// The determinism reference: seed 42, 128x128 cells (512x512 units).
pub fn golden() -> World {
    sandbox(base_config(42, 128, 128), 8, 4, 200)
}

/// The performance reference: a fixed population (no births or deaths) on a
/// grid of stated size, so the cost per tick is a property of the code, not of
/// how the population happened to evolve.
pub fn bench(population: usize, grid: usize) -> World {
    let mut cfg = base_config(7, grid, grid);
    cfg.fixed_population = true;
    cfg.max_population = population;
    sandbox(cfg, 16, 8, population)
}

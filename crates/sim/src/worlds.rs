//! Worlds to design against. A world is a JSON spec, the way an organism is
//! a genome: size, vents, sunlight, and how the food supply changes. Named
//! presets cover the main pressures; `random` rolls a world from the seed.

use crate::rng::Pcg32;
use crate::scenario;
use crate::world::{Dynamics, World};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct WorldSpec {
    pub name: String,
    pub description: String,
    /// Cells per side (4 world units each).
    pub grid: usize,
    pub sugar_vents: usize,
    pub mineral_vents: usize,
    /// Per vent per tick (the mean, when vents pulse).
    pub sugar_rate: f32,
    pub mineral_rate: f32,
    /// Sugar added to every cell every tick.
    pub background_sugar: f32,
    pub diffusion: f32,
    pub decay: f32,
    /// World units a vent wanders per tick.
    pub drift: f32,
    /// Ticks per boom-and-bust cycle; 0 = none.
    pub pulse_period: u32,
    /// 0..=1.
    pub pulse_depth: f32,
    /// Chance per vent per tick of dying and reappearing elsewhere.
    pub relocate_chance: f32,
    pub max_population: usize,
}

impl Default for WorldSpec {
    fn default() -> Self {
        WorldSpec {
            name: "calm".into(),
            description: "Static vents: 8 sugar, 4 mineral. The original sandbox.".into(),
            grid: 128,
            sugar_vents: 8,
            mineral_vents: 4,
            sugar_rate: 3.0,
            mineral_rate: 1.0,
            background_sugar: 0.0,
            diffusion: 0.1,
            decay: 0.0005,
            drift: 0.0,
            pulse_period: 0,
            pulse_depth: 0.0,
            relocate_chance: 0.0,
            max_population: 4000,
        }
    }
}

pub const PRESETS: [&str; 7] = ["calm", "drift", "seasons", "patchy", "desert", "bloom", "random"];

/// A named preset. `random` needs the seed: it is rolled from it.
pub fn preset(name: &str, seed: u64) -> Option<WorldSpec> {
    let base = WorldSpec::default();
    Some(match name {
        "calm" => base,
        "drift" => WorldSpec {
            name: "drift".into(),
            description: "Vents wander about 0.25 units a tick. Sitting still starves; trackers win.".into(),
            drift: 0.25,
            ..base
        },
        "seasons" => WorldSpec {
            name: "seasons".into(),
            description: "Vents pulse on a 3000-tick cycle between 10% and 190% of their mean. Boom and famine.".into(),
            pulse_period: 3000,
            pulse_depth: 0.9,
            ..base
        },
        "patchy" => WorldSpec {
            name: "patchy".into(),
            description: "Each vent dies and reappears somewhere random about every 1500 ticks. Explorers find new hotspots first.".into(),
            relocate_chance: 1.0 / 1500.0,
            ..base
        },
        "desert" => WorldSpec {
            name: "desert".into(),
            description: "Three rich sugar vents and two mineral vents, far apart, in a bigger world. Long trips between oases.".into(),
            grid: 192,
            sugar_vents: 3,
            mineral_vents: 2,
            sugar_rate: 8.0,
            mineral_rate: 2.0,
            ..base
        },
        "bloom" => WorldSpec {
            name: "bloom".into(),
            description: "Weak sugar falls on every cell (sunlight) and vents are few. Grazing anywhere beats hunting vents.".into(),
            sugar_vents: 2,
            background_sugar: 0.0012,
            ..base
        },
        "random" => roll(seed),
        _ => return None,
    })
}

/// A world rolled from the seed. Its own RNG stream, so it never disturbs
/// the world's. The result is a concrete spec, reported to players like
/// any other.
fn roll(seed: u64) -> WorldSpec {
    let mut r = Pcg32::new(seed, 0x0077_0e1d);
    let pick = |r: &mut Pcg32, p: f32| r.next_f32() < p;
    let sugar_vents = 2 + r.below(10) as usize;
    let mineral_vents = 1 + r.below(5) as usize;
    // Keep total sugar inflow in a playable band whatever the vent count.
    let total = r.range(14.0, 34.0);
    WorldSpec {
        name: format!("random-{seed}"),
        description: "Rolled from the match seed: vent count, strength and behaviour are a surprise.".into(),
        grid: [96, 128, 160][r.below(3) as usize],
        sugar_vents,
        mineral_vents,
        sugar_rate: total / sugar_vents as f32,
        mineral_rate: r.range(0.6, 2.0),
        background_sugar: if pick(&mut r, 0.3) { r.range(0.0004, 0.0012) } else { 0.0 },
        drift: if pick(&mut r, 0.5) { r.range(0.05, 0.4) } else { 0.0 },
        pulse_period: if pick(&mut r, 0.4) { 1000 + r.below(4000) } else { 0 },
        pulse_depth: r.range(0.3, 0.95),
        relocate_chance: if pick(&mut r, 0.4) { 1.0 / r.range(800.0, 4000.0) } else { 0.0 },
        ..WorldSpec::default()
    }
}

impl WorldSpec {
    pub fn check(&self) -> Result<(), String> {
        let bad = |w: &str| Err(format!("world spec: {w}"));
        if !(48..=512).contains(&self.grid) {
            return bad("grid must be 48..=512 cells");
        }
        if self.sugar_vents + self.mineral_vents == 0 && self.background_sugar <= 0.0 {
            return bad("no food: give it vents or background_sugar");
        }
        if !(0.0..=0.25).contains(&self.diffusion) {
            return bad("diffusion must be 0..=0.25");
        }
        if !(0.0..=1.0).contains(&self.pulse_depth) || !(0.0..=1.0).contains(&self.relocate_chance) {
            return bad("pulse_depth and relocate_chance must be 0..=1");
        }
        for (n, v) in [("sugar_rate", self.sugar_rate), ("mineral_rate", self.mineral_rate),
                       ("background_sugar", self.background_sugar), ("decay", self.decay), ("drift", self.drift)] {
            if !v.is_finite() || v < 0.0 {
                return bad(&format!("{n} must be a finite number >= 0"));
            }
        }
        Ok(())
    }

    /// An empty world built from this spec.
    pub fn build(&self, seed: u64) -> Result<World, String> {
        self.check()?;
        let mut cfg = scenario::base_config(seed, self.grid, self.grid);
        cfg.diffusion = self.diffusion;
        cfg.decay = self.decay;
        cfg.max_population = self.max_population;
        cfg.dynamics = Dynamics {
            drift: self.drift,
            pulse_period: self.pulse_period,
            pulse_depth: self.pulse_depth,
            relocate_chance: self.relocate_chance,
            background_sugar: self.background_sugar,
        };
        Ok(scenario::pool_with(cfg, self.sugar_vents, self.mineral_vents, self.sugar_rate, self.mineral_rate))
    }
}

/// A preset name or a path to a WorldSpec JSON file.
pub fn load(name_or_path: &str, seed: u64) -> Result<WorldSpec, String> {
    if let Some(s) = preset(name_or_path, seed) {
        return Ok(s);
    }
    let text = std::fs::read_to_string(name_or_path)
        .map_err(|e| format!("{name_or_path}: not a preset ({}) and not a readable file: {e}", PRESETS.join(", ")))?;
    let s: WorldSpec = serde_json::from_str(&text).map_err(|e| format!("{name_or_path}: {e}"))?;
    s.check()?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calm_is_exactly_the_original_sandbox() {
        let a = preset("calm", 42).unwrap().build(42).unwrap();
        let b = scenario::pool(scenario::base_config(42, 128, 128), 8, 4);
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn every_preset_builds_runs_and_is_deterministic() {
        for p in PRESETS {
            let spec = preset(p, 7).unwrap();
            let run = || {
                let mut w = spec.build(7).unwrap();
                let sp = w.add_species(crate::genome::Genome::from_json(scenario::FOUNDER).unwrap()).unwrap();
                w.release(sp, 30).unwrap();
                for _ in 0..600 {
                    w.tick();
                }
                w.hash()
            };
            assert_eq!(run(), run(), "{p} is not deterministic");
        }
    }

    #[test]
    fn dynamic_vents_move_and_their_state_is_hashed() {
        let mut w = preset("drift", 3).unwrap().build(3).unwrap();
        let (x0, y0) = (w.cfg.vents[0].x, w.cfg.vents[0].y);
        let h0 = w.hash();
        for _ in 0..400 {
            w.tick();
        }
        let v = &w.cfg.vents[0];
        let moved = ((v.x - x0).abs() + (v.y - y0).abs()) > 10.0;
        assert!(moved, "vent did not wander: {x0},{y0} -> {},{}", v.x, v.y);
        // Planting a vent shift must change the hash (moving vents are state).
        let before = w.hash();
        w.cfg.vents[0].x += 1.0;
        assert_ne!(before, w.hash());
        assert_ne!(h0, before);
    }

    #[test]
    fn random_worlds_differ_by_seed_and_repeat_by_seed() {
        assert_eq!(preset("random", 5), preset("random", 5));
        assert_ne!(preset("random", 5), preset("random", 6));
        for s in 0..50 {
            preset("random", s).unwrap().check().unwrap();
        }
    }

    #[test]
    fn bad_specs_are_refused_by_rule() {
        let s = WorldSpec { sugar_vents: 0, mineral_vents: 0, ..WorldSpec::default() };
        assert!(s.check().unwrap_err().contains("no food"));
        let e: Result<WorldSpec, _> = serde_json::from_str(r#"{"vents": 3}"#);
        assert!(e.unwrap_err().to_string().contains("unknown field"));
    }
}

//! The genome: a versioned JSON document. The browser's editor and an agent
//! write the same thing. `express` validates it and derives the phenotype;
//! every refusal names the rule that was broken.

use crate::det;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "lifeapi.genome/1";
pub const MAX_PARTS: usize = 16;
pub const MAX_SENSORS: usize = 8;
pub const MAX_HIDDEN: usize = 16;
pub const MIN_RADIUS: f32 = 2.0;
pub const MAX_RADIUS: f32 = 20.0;
/// Brain outputs: [thrust, turn].
pub const N_OUT: usize = 2;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Chemical {
    Sugar,
    Mineral,
    /// Not a chemical: the scent of OTHER teams' organisms (bodies, by area),
    /// so predators can find prey and prey can notice predators. Camouflaged
    /// bodies give off none.
    Organism,
}

impl Chemical {
    pub fn index(self) -> usize {
        match self {
            Chemical::Sugar => 0,
            Chemical::Mineral => 1,
            Chemical::Organism => 2,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum Part {
    /// The body. Exactly one per genome.
    Membrane { radius: f32 },
    /// Forward thrust, 0..=1 each.
    Flagellum { power: f32 },
    /// Turning, 0..=1 each.
    Cilia { power: f32 },
    /// Eats sugar and mineral from the cell it sits in.
    Mouth,
    /// Reads one chemical at a point on the rim, `angle` degrees off the heading.
    ChemoSensor { angle: f32, chemical: Chemical },
    /// Extra energy storage.
    Vacuole { capacity: f32 },
    /// OFFENCE. Bites an enemy organism on contact, if it is smaller than you.
    /// You keep 35% of the energy taken. Shells block up to 75%.
    Jaw { power: f32 },
    /// OFFENCE. Drains any other team's organism on contact, any size, and
    /// pierces shells. You keep only a fifth: it kills more than it feeds.
    Stinger { power: f32 },
    /// DEFENCE. Blocks 75% of jaw bites at full thickness. Heavy: slows you.
    Shell { thickness: f32 },
    /// DEFENCE. Anything that bites or stings you takes damage.
    Spikes { power: f32 },
    /// DEFENCE. Other organisms' sensors can't smell you.
    Camouflage,
    /// DEFENCE. Whoever bites or stings you is poisoned instead of fed.
    Toxin { power: f32 },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BrainSpec {
    pub hidden: u32,
    /// Empty: each founder gets random weights. Otherwise exactly
    /// `Phenotype::n_weights` values.
    #[serde(default)]
    pub weights: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    /// Chance per brain weight per birth of a change.
    pub rate: f32,
    /// Largest change, uniform in [-size, size].
    pub size: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Genome {
    pub schema: String,
    pub name: String,
    pub body: Vec<Part>,
    pub brain: BrainSpec,
    pub mutation: Mutation,
}

/// What the genome builds. Derived, never sent in.
#[derive(Clone, Debug, PartialEq)]
pub struct Phenotype {
    pub radius: f32,
    pub thrust: f32,
    /// Radians per tick at full turn output.
    pub turn: f32,
    pub mouths: u32,
    /// (angle in radians, chemical index)
    pub sensors: Vec<(f32, usize)>,
    pub capacity: f32,
    /// Combat parts (each sum capped at 1).
    pub jaw: f32,
    pub stinger: f32,
    pub shell: f32,
    pub spikes: f32,
    pub camouflage: bool,
    pub toxin: f32,
    /// Has a sensor for other organisms' scent.
    pub smells: bool,
    /// Energy burned per tick just for existing.
    pub upkeep: f32,
    /// Mineral needed to build a child.
    pub child_material: f32,
    /// Energy a released founder starts with: half the BODY's capacity
    /// (2*r^2), never the vacuoles'. Storage is filled by eating. (Arena
    /// match 2: founders starting with half of 15 vacuoles' capacity were
    /// "batteries" that lived ~2500 ticks without eating, for 0.8 budget.)
    pub founder_energy: f32,
    pub hidden: usize,
    pub n_in: usize,
    pub n_weights: usize,
}

impl Genome {
    pub fn from_json(s: &str) -> Result<Genome, String> {
        serde_json::from_str(s).map_err(|e| format!("not a valid {SCHEMA} document: {e}"))
    }

    pub fn express(&self) -> Result<Phenotype, String> {
        if self.schema != SCHEMA {
            return Err(format!("schema must be \"{SCHEMA}\", got \"{}\"", self.schema));
        }
        if self.body.is_empty() || self.body.len() > MAX_PARTS {
            return Err(format!("body must have 1..={MAX_PARTS} parts, has {}", self.body.len()));
        }
        let finite = |what: &str, v: f32| -> Result<(), String> {
            if v.is_finite() {
                Ok(())
            } else {
                Err(format!("{what} must be a finite number"))
            }
        };
        let in_range = |what: &str, v: f32, lo: f32, hi: f32| -> Result<(), String> {
            finite(what, v)?;
            if v < lo || v > hi {
                Err(format!("{what} must be in {lo}..={hi}, got {v}"))
            } else {
                Ok(())
            }
        };

        let mut radius = None;
        let (mut thrust, mut cilia, mut mouths, mut vacuole) = (0.0f32, 0.0f32, 0u32, 0.0f32);
        let (mut jaw, mut stinger, mut shell, mut spikes, mut toxin) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut camouflage = false;
        let mut sensors = Vec::new();
        for (i, p) in self.body.iter().enumerate() {
            match *p {
                Part::Membrane { radius: r } => {
                    if radius.is_some() {
                        return Err(format!("body[{i}]: only one membrane is allowed"));
                    }
                    in_range(&format!("body[{i}].radius"), r, MIN_RADIUS, MAX_RADIUS)?;
                    radius = Some(r);
                }
                Part::Flagellum { power } => {
                    in_range(&format!("body[{i}].power"), power, 0.0, 1.0)?;
                    thrust += power;
                }
                Part::Cilia { power } => {
                    in_range(&format!("body[{i}].power"), power, 0.0, 1.0)?;
                    cilia += power;
                }
                Part::Mouth => mouths += 1,
                Part::ChemoSensor { angle, chemical } => {
                    in_range(&format!("body[{i}].angle"), angle, -180.0, 180.0)?;
                    if sensors.len() == MAX_SENSORS {
                        return Err(format!("at most {MAX_SENSORS} sensors"));
                    }
                    sensors.push((angle * det::DEG, chemical.index()));
                }
                Part::Vacuole { capacity } => {
                    in_range(&format!("body[{i}].capacity"), capacity, 0.0, 100.0)?;
                    vacuole += capacity;
                }
                Part::Jaw { power } => {
                    in_range(&format!("body[{i}].power"), power, 0.0, 1.0)?;
                    jaw += power;
                }
                Part::Stinger { power } => {
                    in_range(&format!("body[{i}].power"), power, 0.0, 1.0)?;
                    stinger += power;
                }
                Part::Shell { thickness } => {
                    in_range(&format!("body[{i}].thickness"), thickness, 0.0, 1.0)?;
                    shell += thickness;
                }
                Part::Spikes { power } => {
                    in_range(&format!("body[{i}].power"), power, 0.0, 1.0)?;
                    spikes += power;
                }
                Part::Camouflage => camouflage = true,
                Part::Toxin { power } => {
                    in_range(&format!("body[{i}].power"), power, 0.0, 1.0)?;
                    toxin += power;
                }
            }
        }
        let r = radius.ok_or("body must have a membrane")?;
        let hidden = self.brain.hidden as usize;
        if hidden == 0 || hidden > MAX_HIDDEN {
            return Err(format!("brain.hidden must be in 1..={MAX_HIDDEN}, got {hidden}"));
        }
        in_range("mutation.rate", self.mutation.rate, 0.0, 1.0)?;
        in_range("mutation.size", self.mutation.size, 0.0, 1.0)?;

        // Inputs: one per sensor, own energy fraction, and a bias of 1.
        let n_in = sensors.len() + 2;
        let n_weights = n_in * hidden + (hidden + 1) * N_OUT;
        if !self.brain.weights.is_empty() {
            if self.brain.weights.len() != n_weights {
                return Err(format!(
                    "brain.weights must be empty or have exactly {n_weights} values \
                     ({n_in} inputs x {hidden} hidden + ({hidden} + 1) x {N_OUT} outputs), got {}",
                    self.brain.weights.len()
                ));
            }
            for (i, w) in self.brain.weights.iter().enumerate() {
                in_range(&format!("brain.weights[{i}]"), *w, -4.0, 4.0)?;
            }
        }

        let area = r * r;
        let (jaw, stinger, shell, spikes, toxin) =
            (det::min(jaw, 1.0), det::min(stinger, 1.0), det::min(shell, 1.0), det::min(spikes, 1.0), det::min(toxin, 1.0));
        let smells = sensors.iter().any(|&(_, c)| c == Chemical::Organism.index());
        Ok(Phenotype {
            radius: r,
            // A shell is heavy: full thickness costs 40% of your speed.
            thrust: thrust * (1.0 - 0.4 * det::min(shell, 1.0)),
            turn: 0.05 + 0.15 * cilia,
            mouths,
            capacity: 2.0 * area + vacuole,
            upkeep: 0.0005 * area
                + 0.01 * thrust
                + 0.006 * cilia
                + 0.004 * mouths as f32
                + 0.002 * sensors.len() as f32
                + 0.0002 * vacuole
                + 0.010 * jaw
                + 0.006 * stinger
                + 0.002 * shell
                + 0.004 * spikes
                + if camouflage { 0.006 } else { 0.0 }
                + 0.004 * toxin,
            jaw,
            stinger,
            shell,
            spikes,
            camouflage,
            toxin,
            smells,
            child_material: 0.1 * area,
            founder_energy: area,
            sensors,
            hidden,
            n_in,
            n_weights,
        })
    }
}

/// What a player needs to write a genome, from the same constants
/// `express` enforces. The costs are given as the formulas in `express`.
pub fn catalog() -> serde_json::Value {
    serde_json::json!({
        "schema": SCHEMA,
        "limits": {
            "max_parts": MAX_PARTS,
            "max_sensors": MAX_SENSORS,
            "max_hidden": MAX_HIDDEN,
            "radius": [MIN_RADIUS, MAX_RADIUS],
            "brain_weight": [-4.0, 4.0],
            "mutation_rate": [0.0, 1.0],
            "mutation_size": [0.0, 1.0],
        },
        "parts": [
            {"part": "membrane", "fields": {"radius": "number, the body; exactly one"}},
            {"part": "flagellum", "fields": {"power": "0..=1, forward thrust"}},
            {"part": "cilia", "fields": {"power": "0..=1, turning"}},
            {"part": "mouth", "fields": {}, "note": "eats sugar (energy) and mineral (material for children) from the cell under the body"},
            {"part": "chemo_sensor", "fields": {"angle": "-180..=180 degrees off the heading", "chemical": "sugar | mineral"}},
            {"part": "vacuole", "fields": {"capacity": "0..=100 extra energy storage"}},
            {"part": "jaw", "fields": {"power": "0..=1"}, "role": "offence",
             "note": "on contact, bites an enemy organism SMALLER than you: takes 0.8*power energy per tick (less 75%*thickness of its shell) and some material; you keep 35%"},
            {"part": "stinger", "fields": {"power": "0..=1"}, "role": "offence",
             "note": "on contact, drains 0.4*power energy per tick from an enemy organism of ANY size, ignoring shells; you keep only a fifth"},
            {"part": "shell", "fields": {"thickness": "0..=1"}, "role": "defence",
             "note": "jaw bites on you are cut by 75%*thickness; costs up to 40% of your speed"},
            {"part": "spikes", "fields": {"power": "0..=1"}, "role": "defence",
             "note": "anything that bites or stings you loses 1.0*power energy per tick of contact (more than a jaw gains)"},
            {"part": "camouflage", "fields": {}, "role": "defence",
             "note": "other organisms' 'organism' sensors can't smell you"},
            {"part": "toxin", "fields": {"power": "0..=1"}, "role": "defence",
             "note": "whoever bites or stings you gains less, and at power >= 0.5 LOSES energy instead (gain x (1 - 2*power))"},
        ],
        "chemicals": ["sugar", "mineral", "organism"],
        "combat": "who counts as an enemy depends on the match's combat setting (told at hello): off = no combat; \
                   colony = other players' organisms; individual = any organism except your parent, children and \
                   siblings. Bodies touch when they overlap. A sensor with chemical \"organism\" smells enemy \
                   (non-camouflaged) bodies nearby.",
        "brain": {
            "inputs": "one per chemo_sensor (reading v as v/(v+1)), then own energy/capacity, then a bias of 1",
            "outputs": ["thrust (negative = stop)", "turn"],
            "activation": "tanh",
            "weight_layout": "hidden rows [hidden][inputs], then output rows [2][hidden+1] with the output bias last",
            "n_weights": "inputs*hidden + (hidden+1)*2",
        },
        "phenotype": {
            "capacity": "2*r^2 + sum(vacuole.capacity)",
            "upkeep_per_tick": "0.0005*r^2 + 0.01*thrust + 0.006*cilia + 0.004*mouths + 0.002*sensors + 0.0002*vacuole \
                                + 0.01*jaw + 0.006*stinger + 0.002*shell + 0.004*spikes + 0.006*camouflage + 0.004*toxin",
            "turn_rad_per_tick": "0.05 + 0.15*sum(cilia.power)",
            "speed": "max_speed * max(thrust_out,0) * sum(flagellum.power) * 6/r",
            "child_material": "0.1*r^2",
            "divides_when": "energy >= split_frac*capacity and material >= child_material; the child gets half the energy and a mutated brain",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::FOUNDER;

    #[test]
    fn founder_expresses() {
        let p = Genome::from_json(FOUNDER).unwrap().express().unwrap();
        assert_eq!(p.radius, 6.0);
        assert_eq!(p.sensors.len(), 2);
        assert_eq!(p.n_in, 4);
    }

    /// Match 2's "battery": radius 2, 15 vacuoles, nothing else. It must
    /// start with the body's energy only, so it can't live for thousands of
    /// ticks without eating.
    #[test]
    fn vacuoles_start_empty() {
        let battery = r#"{"schema":"lifeapi.genome/1","name":"batt","body":[{"part":"membrane","radius":2},
            {"part":"vacuole","capacity":100},{"part":"vacuole","capacity":100},{"part":"vacuole","capacity":100}],
            "brain":{"hidden":1},"mutation":{"rate":0,"size":0}}"#;
        let p = Genome::from_json(battery).unwrap().express().unwrap();
        assert_eq!(p.founder_energy, 4.0);
        assert!(p.capacity > 300.0);
        // Lifespan on starting energy alone: a few dozen ticks, not thousands.
        assert!(p.founder_energy / p.upkeep < 100.0, "{}", p.founder_energy / p.upkeep);
    }

    #[test]
    fn refusals_name_the_rule() {
        let bad = FOUNDER.replace("\"radius\":6", "\"radius\":60");
        let e = Genome::from_json(&bad).unwrap().express().unwrap_err();
        assert!(e.contains("radius") && e.contains("20"), "{e}");

        let bad = FOUNDER.replace("\"hidden\":6", "\"hidden\":6,\"weights\":[1.0]");
        let e = Genome::from_json(&bad).unwrap().express().unwrap_err();
        // 4 inputs x 6 hidden + (6 + 1) x 2 outputs = 38.
        assert!(e.contains("exactly 38"), "{e}");

        let e = Genome::from_json("{\"schema\":1}").unwrap_err();
        assert!(e.contains(SCHEMA), "{e}");
    }
}

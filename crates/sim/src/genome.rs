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
}

impl Chemical {
    pub fn index(self) -> usize {
        match self {
            Chemical::Sugar => 0,
            Chemical::Mineral => 1,
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
    /// Energy burned per tick just for existing.
    pub upkeep: f32,
    /// Mineral needed to build a child.
    pub child_material: f32,
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
        Ok(Phenotype {
            radius: r,
            thrust,
            turn: 0.05 + 0.15 * cilia,
            mouths,
            capacity: 2.0 * area + vacuole,
            upkeep: 0.0005 * area
                + 0.01 * thrust
                + 0.006 * cilia
                + 0.004 * mouths as f32
                + 0.002 * sensors.len() as f32
                + 0.0002 * vacuole,
            child_material: 0.1 * area,
            sensors,
            hidden,
            n_in,
            n_weights,
        })
    }
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

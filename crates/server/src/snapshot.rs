//! What a spectator sees: a JSON "meta" message plus a binary "frame".
//!
//! Sandbox spectating shows the whole pool. Arena matches (M4) will close
//! this stream to their players and delay it for everyone else (design §6).
//!
//! Binary frame, little-endian, version 2:
//! ```text
//! 0   "LAPI"           4 bytes
//! 4   version u8 = 1, then 3 zero bytes
//! 8   tick u64
//! 16  grid_w u16, grid_h u16   (the chemical field, downsampled to <= 256)
//! 20  world_w f32, world_h f32
//! 28  n u32                    organisms
//! 32  sugar   u8[grid_w*grid_h]   v/(v+1)*255, row-major
//!     mineral u8[grid_w*grid_h]
//!     n x 24 bytes: x f32, y f32, heading f32, lineage u32,
//!                   energy u16 (fraction of capacity * 65535),
//!                   radius u16 (radius * 100), id u32 (low 32 bits of the
//!                   organism id, for tracking one organism)
//! ```

use crate::Game;
use serde_json::json;

pub const MAGIC: &[u8; 4] = b"LAPI";
pub const FRAME_VERSION: u8 = 2;
pub const HEADER_BYTES: usize = 32;
pub const ORG_BYTES: usize = 24;
pub const MAX_FIELD: usize = 256;

pub struct Snapshot {
    pub version: u64,
    /// The tick of the newest history point included (for the next request).
    pub history_tick: Option<u64>,
    pub meta: String,
    pub frame: Vec<u8>,
}

pub fn build(game: &Game, history_after: Option<u64>) -> Snapshot {
    let w = game.world();
    let counts = game.lineage_counts();
    let mut lineages = Vec::new();
    // Every lineage that ever lived, so extinct ones show as 0.
    let mut seen = vec![false; counts.len()];
    for o in &w.orgs {
        let l = o.lineage as usize;
        if !seen[l] {
            seen[l] = true;
            let sp = &w.species[o.species as usize];
            lineages.push(json!({
                "lineage": o.lineage,
                "species": o.species,
                "species_name": sp.genome.name,
                "body": body_plan(&sp.genome),
                "owner": game.owner_name(o.lineage),
                "alive": counts[l],
            }));
        }
    }
    for (l, s) in seen.iter().enumerate().skip(1) {
        if !*s && game.owner_name(l as u32).is_some() {
            lineages.push(json!({"lineage": l, "owner": game.owner_name(l as u32), "alive": 0}));
        }
    }
    let history: Vec<_> = game
        .history()
        .iter()
        .filter(|p| history_after.map_or(true, |t| p.tick > t))
        .collect();
    let history_tick = history.last().map(|p| p.tick).or(history_after);
    let meta = json!({
        "type": "meta",
        "tick": w.tick,
        "history_reset": history_after.is_none(),
        "world": {
            "width": w.grid.world_w(),
            "height": w.grid.world_h(),
            "population": w.orgs.len(),
            "births": w.stats.births,
            "deaths": w.stats.deaths,
        },
        "lineages": lineages,
        "history": history,
    })
    .to_string();
    Snapshot { version: game.version(), history_tick, meta, frame: encode_frame(game) }
}

/// One organism for the viewer's track panel: state, what it senses, and
/// what its brain decides for the next tick. None if it is gone.
pub fn inspect(game: &Game, id: u64) -> Option<String> {
    let w = game.world();
    // The frame carries the low 32 bits of the id; match on those.
    let full = w.orgs.iter().find(|o| o.id == id || o.id as u32 as u64 == id)?.id;
    let i = w.inspect(full)?;
    let sensors: Vec<_> = i
        .sensors
        .iter()
        .map(|&(ang, c, v)| json!({"angle_deg": (ang / lifeapi_sim::det::DEG).round(),
                                   "chemical": if c == 0 { "sugar" } else { "mineral" },
                                   "reading": v, "input": v / (v + 1.0)}))
        .collect();
    Some(
        json!({
            "id": i.id, "lineage": i.lineage, "species": i.name, "owner": game.owner_name(i.lineage),
            "generation": i.generation, "age": i.age, "tick": w.tick,
            "x": i.x, "y": i.y, "heading_deg": (i.heading / lifeapi_sim::det::DEG).round(),
            "energy": i.energy, "capacity": i.capacity, "split_energy": i.split_energy,
            "material": i.material, "child_material": i.child_material, "upkeep": i.upkeep,
            "mouths": i.mouths, "sugar_here": i.sugar_here, "mineral_here": i.mineral_here,
            "sensors": sensors,
            "brain": {"thrust": i.thrust_out, "turn": i.turn_out},
            "next": {"speed": i.speed, "turn_deg": (i.turn_rate / lifeapi_sim::det::DEG * 10.0).round() / 10.0},
        })
        .to_string(),
    )
}

/// The parts the viewer draws: flagella (power each), cilia (power each),
/// mouths, sensors ([angle in degrees, chemical]) and vacuoles (capacity each).
fn body_plan(g: &lifeapi_sim::genome::Genome) -> serde_json::Value {
    use lifeapi_sim::genome::{Chemical, Part};
    let (mut flagella, mut cilia, mut sensors, mut vacuoles, mut mouths) = (vec![], vec![], vec![], vec![], 0);
    for part in &g.body {
        match part {
            Part::Membrane { .. } => {}
            Part::Flagellum { power } => flagella.push(*power),
            Part::Cilia { power } => cilia.push(*power),
            Part::Mouth => mouths += 1,
            Part::ChemoSensor { angle, chemical } => {
                sensors.push(json!([angle, if *chemical == Chemical::Sugar { "sugar" } else { "mineral" }]))
            }
            Part::Vacuole { capacity } => vacuoles.push(*capacity),
        }
    }
    json!({"flagella": flagella, "cilia": cilia, "mouths": mouths, "sensors": sensors, "vacuoles": vacuoles})
}

fn encode_frame(game: &Game) -> Vec<u8> {
    let w = game.world();
    let g = &w.grid;
    let step = g.w.max(g.h).div_ceil(MAX_FIELD).max(1);
    let (fw, fh) = (g.w / step, g.h / step);
    let n = w.orgs.len();
    let mut b = Vec::with_capacity(HEADER_BYTES + 2 * fw * fh + n * ORG_BYTES);
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&[FRAME_VERSION, 0, 0, 0]);
    b.extend_from_slice(&w.tick.to_le_bytes());
    b.extend_from_slice(&(fw as u16).to_le_bytes());
    b.extend_from_slice(&(fh as u16).to_le_bytes());
    b.extend_from_slice(&g.world_w().to_le_bytes());
    b.extend_from_slice(&g.world_h().to_le_bytes());
    b.extend_from_slice(&(n as u32).to_le_bytes());
    for c in 0..2 {
        for y in 0..fh {
            for x in 0..fw {
                let v = g.chem[c][(y * step) * g.w + x * step];
                b.push((v / (v + 1.0) * 255.0) as u8);
            }
        }
    }
    for o in &w.orgs {
        let p = &w.species[o.species as usize].phen;
        b.extend_from_slice(&o.x.to_le_bytes());
        b.extend_from_slice(&o.y.to_le_bytes());
        b.extend_from_slice(&o.heading.to_le_bytes());
        b.extend_from_slice(&o.lineage.to_le_bytes());
        let e = (o.energy / p.capacity).clamp(0.0, 1.0);
        b.extend_from_slice(&((e * 65535.0) as u16).to_le_bytes());
        b.extend_from_slice(&((p.radius * 100.0) as u16).to_le_bytes());
        b.extend_from_slice(&(o.id as u32).to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifeapi_sim::scenario;

    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }
    fn f32_at(b: &[u8], o: usize) -> f32 {
        f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    #[test]
    fn frame_layout_matches_the_documented_one() {
        let mut g = Game::new(scenario::sandbox(scenario::base_config(3, 128, 128), 4, 2, 10));
        g.handle(None, r#"{"op":"hello","name":"v"}"#);
        let s = build(&g, None);
        let b = &s.frame;
        assert_eq!(&b[0..4], MAGIC);
        assert_eq!(b[4], FRAME_VERSION);
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 0);
        let (fw, fh) = (u16::from_le_bytes([b[16], b[17]]) as usize, u16::from_le_bytes([b[18], b[19]]) as usize);
        assert_eq!((fw, fh), (128, 128));
        assert_eq!(f32_at(b, 20), 512.0);
        let n = u32_at(b, 28) as usize;
        assert_eq!(n, 10);
        assert_eq!(b.len(), HEADER_BYTES + 2 * fw * fh + n * ORG_BYTES);
        let o0 = HEADER_BYTES + 2 * fw * fh;
        let org = &g.world().orgs[0];
        assert_eq!(f32_at(b, o0), org.x);
        assert_eq!(f32_at(b, o0 + 4), org.y);
        assert_eq!(u32_at(b, o0 + 12), org.lineage);
        assert_eq!(u16::from_le_bytes([b[o0 + 18], b[o0 + 19]]), 600);
        assert_eq!(u32_at(b, o0 + 20) as u64, org.id);
        // The track panel: found by id, and gone ids say so.
        let i: serde_json::Value = serde_json::from_str(&inspect(&g, org.id).unwrap()).unwrap();
        assert_eq!(i["id"], org.id);
        assert!(i["brain"]["thrust"].is_number() && i["sensors"].is_array());
        assert!(inspect(&g, 999_999).is_none());
        let meta: serde_json::Value = serde_json::from_str(&s.meta).unwrap();
        assert_eq!(meta["history_reset"], true);
        assert_eq!(meta["world"]["population"], 10);
    }

    #[test]
    fn big_grids_are_downsampled_to_at_most_256() {
        let g = Game::new(scenario::pool(scenario::base_config(3, 1024, 1024), 4, 2));
        let b = build(&g, None).frame;
        assert_eq!(u16::from_le_bytes([b[16], b[17]]), 256);
        assert_eq!(b.len(), HEADER_BYTES + 2 * 256 * 256);
    }

    #[test]
    fn history_is_sent_only_after_the_given_tick() {
        let mut g = Game::new(scenario::sandbox(scenario::base_config(3, 64, 64), 4, 2, 10));
        let (_, p) = g.handle(None, r#"{"op":"hello","name":"v"}"#);
        g.handle(p, r#"{"op":"step","ticks":200}"#);
        let all: serde_json::Value = serde_json::from_str(&build(&g, None).meta).unwrap();
        assert_eq!(all["history"].as_array().unwrap().len(), 5); // ticks 0,50,100,150,200
        let s = build(&g, Some(100));
        let some: serde_json::Value = serde_json::from_str(&s.meta).unwrap();
        assert_eq!(some["history"].as_array().unwrap().len(), 2);
        assert_eq!(some["history_reset"], false);
        assert_eq!(s.history_tick, Some(200));
    }
}

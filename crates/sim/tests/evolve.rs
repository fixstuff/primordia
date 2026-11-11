//! Body evolution: a setting. Off, nothing changes; on, children's bodies
//! drift into new species that stay valid, on the parent's team and lineage.

use primordia_sim::scenario;
use primordia_sim::world::{CombatMode, World};

fn run(rate: f32, combat: CombatMode, seed: u64) -> World {
    let mut cfg = scenario::base_config(seed, 64, 64);
    cfg.body_mutation = rate;
    cfg.combat = combat;
    let mut w = scenario::sandbox(cfg, 4, 2, 40);
    for s in w.species.iter_mut() {
        s.team = 1;
    }
    for _ in 0..3000 {
        w.tick();
    }
    w
}

#[test]
fn off_means_one_species_forever() {
    let w = run(0.0, CombatMode::Colony, 4);
    assert_eq!(w.species.len(), 1);
}

#[test]
fn on_bodies_evolve_into_valid_species_and_brains_fit_them() {
    let w = run(0.2, CombatMode::Colony, 4);
    assert!(w.species.len() > 5, "species: {}", w.species.len());
    assert!(w.orgs.iter().any(|o| o.species != 0), "some living organism has an evolved body");
    for s in &w.species {
        let p = s.genome.express().expect("every evolved body is valid");
        assert_eq!(p, s.phen);
        assert_eq!(s.team, 1, "evolved species stay on the parent's team");
    }
    for o in &w.orgs {
        let p = &w.species[o.species as usize].phen;
        assert_eq!(o.brain.len(), p.n_weights, "org {}'s brain fits its body", o.id);
        assert_eq!(o.lineage, 1, "evolution keeps the family line");
    }
    // Identical bodies on a team share one species.
    for (i, a) in w.species.iter().enumerate() {
        for b in &w.species[i + 1..] {
            assert!(a.genome.body != b.genome.body || a.genome.brain.hidden != b.genome.brain.hidden);
        }
    }
}

#[test]
fn evolution_is_deterministic_and_respects_combat_off() {
    assert_eq!(run(0.2, CombatMode::Colony, 7).hash(), run(0.2, CombatMode::Colony, 7).hash());
    let w = run(0.3, CombatMode::Off, 7);
    for s in &w.species {
        let p = &s.phen;
        assert!(p.jaw == 0.0 && p.stinger == 0.0 && p.shell == 0.0 && p.spikes == 0.0 && p.toxin == 0.0 && !p.camouflage,
                "combat off: evolution never adds combat parts ({})", s.genome.name);
        assert!(!p.smells, "combat off: no organism sensors evolve");
    }
}

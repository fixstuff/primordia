//! Offence and defence: each rule, one test, with organisms held still and
//! touching so contact is certain. Bodies have no flagella (they can't move)
//! and no mouths (sugar never confuses the energy books).

use lifeapi_sim::genome::Genome;
use lifeapi_sim::scenario;
use lifeapi_sim::world::{World, JAW_KEEP, JAW_RATE, SHELL_BLOCK, SPIKE_RATE, STING_KEEP, STING_RATE};

fn genome(name: &str, radius: f32, extra: &str) -> Genome {
    let extra = if extra.is_empty() { String::new() } else { format!(",{extra}") };
    Genome::from_json(&format!(
        r#"{{"schema":"lifeapi.genome/1","name":"{name}","body":[{{"part":"membrane","radius":{radius}}}{extra}],
            "brain":{{"hidden":1}},"mutation":{{"rate":0,"size":0}}}}"#
    ))
    .unwrap()
}

/// A world with organism A (team ta) at (100,100) and B (team tb) just
/// touching it to the right. Returns the world and both ids.
fn duel(a: Genome, ta: u32, b: Genome, tb: u32) -> (World, u64, u64) {
    let mut w = scenario::pool(scenario::base_config(1, 64, 64), 0, 0);
    let (ra, rb) = (a.express().unwrap().radius, b.express().unwrap().radius);
    let sa = w.add_species(a).unwrap();
    let sb = w.add_species(b).unwrap();
    w.species[sa as usize].team = ta;
    w.species[sb as usize].team = tb;
    w.release(sa, 1).unwrap();
    w.release(sb, 1).unwrap();
    let (ia, ib) = (w.orgs[0].id, w.orgs[1].id);
    for (k, (x, e)) in [(100.0, 10.0), (100.0 + ra + rb, 10.0)].into_iter().enumerate() {
        w.orgs[k].x = x;
        w.orgs[k].y = 100.0;
        w.orgs[k].energy = e;
    }
    (w, ia, ib)
}

fn energy(w: &World, id: u64) -> Option<f32> {
    w.orgs.iter().find(|o| o.id == id).map(|o| o.energy)
}

/// Energy change over one tick, net of upkeep (the organism's own cost).
fn delta(w: &World, id: u64, before: f32) -> f32 {
    let o = w.orgs.iter().find(|o| o.id == id).unwrap();
    o.energy - before + w.species[o.species as usize].phen.upkeep
}

#[test]
fn a_jaw_bites_a_smaller_enemy_and_keeps_half() {
    let (mut w, a, b) = duel(genome("croc", 8.0, r#"{"part":"jaw","power":1}"#), 1, genome("fish", 4.0, ""), 2);
    w.tick();
    assert!((delta(&w, b, 10.0) + JAW_RATE).abs() < 1e-4, "prey loses a bite: {}", delta(&w, b, 10.0));
    assert!((delta(&w, a, 10.0) - JAW_RATE * JAW_KEEP).abs() < 1e-4, "predator keeps its share: {}", delta(&w, a, 10.0));
    let f = &w.orgs.iter().find(|o| o.id == a).unwrap().fight;
    assert_eq!(f.target, b);
}

#[test]
fn no_combat_within_a_team_or_against_a_bigger_body() {
    let (mut w, _, b) = duel(genome("croc", 8.0, r#"{"part":"jaw","power":1}"#), 1, genome("fish", 4.0, ""), 1);
    w.tick();
    assert!(delta(&w, b, 10.0).abs() < 1e-4, "same team: no bite");
    let (mut w, _, b) = duel(genome("small", 4.0, r#"{"part":"jaw","power":1}"#), 1, genome("big", 8.0, ""), 2);
    w.tick();
    assert!(delta(&w, b, 10.0).abs() < 1e-4, "a jaw can't bite a bigger body");
}

#[test]
fn a_shell_halves_a_jaw_but_not_a_stinger() {
    let shell = r#"{"part":"shell","thickness":1}"#;
    let (mut w, _, b) = duel(genome("croc", 8.0, r#"{"part":"jaw","power":1}"#), 1, genome("turtle", 4.0, shell), 2);
    w.tick();
    assert!((delta(&w, b, 10.0) + JAW_RATE * (1.0 - SHELL_BLOCK)).abs() < 1e-4, "shell blocks most of the bite: {}", delta(&w, b, 10.0));
    let (mut w, a, b) = duel(genome("wasp", 3.0, r#"{"part":"stinger","power":1}"#), 1, genome("turtle", 4.0, shell), 2);
    w.tick();
    assert!((delta(&w, b, 10.0) + STING_RATE).abs() < 1e-4, "stinger pierces, any size: {}", delta(&w, b, 10.0));
    assert!((delta(&w, a, 10.0) - STING_RATE * STING_KEEP).abs() < 1e-4, "stinger keeps a little: {}", delta(&w, a, 10.0));
}

#[test]
fn spikes_hurt_the_attacker_and_toxin_poisons_it() {
    let jaw = r#"{"part":"jaw","power":1}"#;
    let (mut w, a, _) = duel(genome("croc", 8.0, jaw), 1, genome("urchin", 4.0, r#"{"part":"spikes","power":1}"#), 2);
    w.tick();
    let want = JAW_RATE * JAW_KEEP - SPIKE_RATE;
    assert!(want < 0.0, "biting an urchin is a net loss");
    assert!((delta(&w, a, 10.0) - want).abs() < 1e-4, "kept a share, lost more to spikes: {}", delta(&w, a, 10.0));
    let (mut w, a, _) = duel(genome("croc", 8.0, jaw), 1, genome("frog", 4.0, r#"{"part":"toxin","power":1}"#), 2);
    w.tick();
    assert!((delta(&w, a, 10.0) + JAW_RATE * JAW_KEEP).abs() < 1e-4, "full toxin: the bite costs what it would have fed: {}", delta(&w, a, 10.0));
}

#[test]
fn prey_killed_in_combat_counts_as_killed_not_starved() {
    let (mut w, _, b) = duel(genome("croc", 8.0, r#"{"part":"jaw","power":1}"#), 1, genome("fish", 4.0, ""), 2);
    let k = w.orgs.iter().position(|o| o.id == b).unwrap();
    w.orgs[k].energy = 0.5;
    let lb = w.orgs[k].lineage as usize;
    w.tick();
    assert!(energy(&w, b).is_none(), "prey is dead");
    assert_eq!((w.lineages[lb].killed, w.lineages[lb].starved), (1, 0));
}

#[test]
fn organism_sensors_smell_other_teams_not_kin_and_not_camouflage() {
    let nose = r#"{"part":"chemo_sensor","angle":0,"chemical":"organism"}"#;
    let reading = |other_team: u32, extra: &str| {
        let (mut w, a, _) = duel(genome("nose", 4.0, nose), 1, genome("other", 6.0, extra), other_team);
        w.tick();
        let i = w.inspect(a).unwrap();
        i.sensors[0].2
    };
    assert!(reading(2, "") > 0.5, "smells the other team's body: {}", reading(2, ""));
    assert_eq!(reading(1, ""), 0.0, "kin give off no scent to their own team");
    assert_eq!(reading(2, r#"{"part":"camouflage"}"#), 0.0, "camouflage hides it");
}

#[test]
fn combat_worlds_are_deterministic_and_team_zero_never_fights() {
    let run = |team: u32| {
        let mut w = scenario::sandbox(scenario::base_config(9, 64, 64), 4, 2, 30);
        let g = genome("croc", 9.0, r#"{"part":"jaw","power":1},{"part":"flagellum","power":1},{"part":"chemo_sensor","angle":20,"chemical":"organism"},{"part":"chemo_sensor","angle":-20,"chemical":"organism"}"#);
        let s = w.add_species(g).unwrap();
        w.species[s as usize].team = team;
        w.release(s, 20).unwrap();
        for _ in 0..400 {
            w.tick();
        }
        let killed: u64 = w.lineages.iter().map(|l| l.killed).sum();
        (w.hash(), killed)
    };
    let (h1, k1) = run(1);
    assert_eq!(run(1), (h1.clone(), k1), "same seed, same result");
    assert!(k1 > 0, "a jawed team on the founders' pool makes kills");
    let (_, k0) = run(0);
    assert_eq!(k0, 0, "everyone on team 0: no combat");
}

use lifeapi_sim::world::CombatMode;

/// Two organisms of the SAME team (one player), predator bigger, touching.
fn kin_duel(mode: CombatMode, parent_a: u64, parent_b: u64, b_child_of_a: bool) -> f32 {
    let (mut w, a, b) = duel(genome("croc", 8.0, r#"{"part":"jaw","power":1}"#), 1, genome("fish", 4.0, ""), 1);
    w.cfg.combat = mode;
    let ka = w.orgs.iter().position(|o| o.id == a).unwrap();
    let kb = w.orgs.iter().position(|o| o.id == b).unwrap();
    w.orgs[ka].parent = parent_a;
    w.orgs[kb].parent = if b_child_of_a { a } else { parent_b };
    w.tick();
    delta(&w, b, 10.0)
}

#[test]
fn colony_spares_your_own_individual_spares_only_family() {
    // Colony: one player's organisms never fight, related or not.
    assert!(kin_duel(CombatMode::Colony, 0, 0, false).abs() < 1e-4);
    // Individual: strangers on the same team do fight...
    assert!((kin_duel(CombatMode::Individual, 0, 0, false) + JAW_RATE).abs() < 1e-4, "strangers fight");
    assert!((kin_duel(CombatMode::Individual, 900, 901, false) + JAW_RATE).abs() < 1e-4, "cousins fight");
    // ...but never parent and child, or siblings.
    assert!(kin_duel(CombatMode::Individual, 0, 0, true).abs() < 1e-4, "parent spares child");
    assert!(kin_duel(CombatMode::Individual, 900, 900, false).abs() < 1e-4, "siblings spare each other");
    // Off: nothing fights at all, even different teams.
    let (mut w, _, b) = duel(genome("croc", 8.0, r#"{"part":"jaw","power":1}"#), 1, genome("fish", 4.0, ""), 2);
    w.cfg.combat = CombatMode::Off;
    w.tick();
    assert!(delta(&w, b, 10.0).abs() < 1e-4, "combat off");
}

#[test]
fn individual_sensors_smell_your_own_lineage_but_not_yourself() {
    let nose = r#"{"part":"chemo_sensor","angle":0,"chemical":"organism"}"#;
    let (mut w, a, _) = duel(genome("nose", 4.0, nose), 1, genome("kin", 6.0, ""), 1);
    w.cfg.combat = CombatMode::Individual;
    w.tick();
    assert!(w.inspect(a).unwrap().sensors[0].2 > 0.5, "smells a same-team neighbour");
    // Alone in the world, it smells nothing: its own body is subtracted.
    let mut w = scenario::pool(scenario::base_config(1, 64, 64), 0, 0);
    let s = w.add_species(genome("nose", 4.0, nose)).unwrap();
    w.cfg.combat = CombatMode::Individual;
    w.release(s, 1).unwrap();
    let id = w.orgs[0].id;
    w.tick();
    assert_eq!(w.inspect(id).unwrap().sensors[0].2, 0.0);
}

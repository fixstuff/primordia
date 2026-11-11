//! `World::inspect` must predict what the next tick does, and change nothing.

use primordia_sim::{det, scenario};

#[test]
fn inspect_predicts_the_next_heading_and_changes_nothing() {
    let mut w = scenario::sandbox(scenario::base_config(5, 64, 64), 4, 2, 40);
    for _ in 0..50 {
        w.tick();
    }
    let before = w.hash();
    let ids: Vec<u64> = w.orgs.iter().map(|o| o.id).take(10).collect();
    let seen: Vec<_> = ids.iter().map(|&id| w.inspect(id).expect("alive")).collect();
    assert_eq!(w.hash(), before, "inspect must not change the world");
    assert!(w.inspect(u64::MAX).is_none());

    w.step_organisms(); // the same sense/think/act step tick() runs first
    for s in &seen {
        let o = w.orgs.iter().find(|o| o.id == s.id).unwrap();
        let want = det::wrap_angle(s.heading + s.turn_rate);
        assert!((o.heading - want).abs() < 1e-6, "org {}: heading {} vs predicted {}", s.id, o.heading, want);
        assert!((-1.0..=1.0).contains(&s.thrust_out) && (-1.0..=1.0).contains(&s.turn_out));
        assert_eq!(s.sensors.len(), w.species[o.species as usize].phen.sensors.len());
    }
}

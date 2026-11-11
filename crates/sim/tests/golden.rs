//! Determinism. The golden hash is committed in `golden_hash.txt`; every
//! build (debug, release) on every machine must reproduce it. Running the
//! same seed twice in one process only catches in-process nondeterminism;
//! the committed file is what catches drift between machines and builds.
//!
//! To re-bless after an INTENDED change to the simulation:
//!   LIFEAPI_BLESS=1 cargo test --release -p lifeapi-sim --test golden
//! and say why in the commit.

use lifeapi_sim::scenario;

const GOLDEN_TICKS: u64 = 10_000;
const GOLDEN_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden_hash.txt");

fn run(ticks: u64) -> lifeapi_sim::World {
    let mut w = scenario::golden();
    for _ in 0..ticks {
        w.tick();
    }
    w
}

#[test]
fn golden_hash_matches() {
    let w = run(GOLDEN_TICKS);
    assert!(
        w.orgs.len() > 50 && w.stats.births > 1000 && w.stats.deaths > 1000,
        "the golden world must be alive and turning over, or its hash proves little: \
         pop {} births {} deaths {}",
        w.orgs.len(),
        w.stats.births,
        w.stats.deaths
    );
    let got = w.hash();
    if std::env::var("LIFEAPI_BLESS").as_deref() == Ok("1") {
        std::fs::write(GOLDEN_FILE, format!("{got}\n")).unwrap();
        eprintln!("blessed {got}");
        return;
    }
    let want = std::fs::read_to_string(GOLDEN_FILE).expect("tests/golden_hash.txt");
    assert_eq!(
        got,
        want.trim(),
        "golden hash differs at tick {GOLDEN_TICKS}: this build/machine does not reproduce the reference world"
    );
}

/// The per-lineage counters (outside the hash) must agree with the world.
#[test]
fn lineage_counters_agree_with_the_world() {
    let mut w = scenario::golden();
    let sp = w.species.len() as u32 - 1;
    w.release(sp, 40).unwrap(); // a second lineage, so there's more than one to check
    for _ in 0..3_000 {
        w.tick();
    }
    let mut live = vec![0u32; w.lineages.len()];
    for o in &w.orgs {
        live[o.lineage as usize] += 1;
    }
    for (id, ls) in w.lineages.iter().enumerate().skip(1) {
        assert_eq!(ls.alive, live[id], "lineage {id}");
        assert_eq!(ls.released + ls.born - ls.starved, ls.alive as u64, "lineage {id}");
        assert!(ls.org_ticks > 0 && ls.energy_in > 0.0, "lineage {id}: {ls:?}");
    }
    let (born, starved): (u64, u64) = w.lineages.iter().map(|l| (l.born, l.starved)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    assert_eq!((born, starved), (w.stats.births, w.stats.deaths));
}

#[test]
fn same_seed_twice_same_hash() {
    assert_eq!(run(2_000).hash(), run(2_000).hash());
}

/// The planted divergence: one organism's energy one ulp off at tick 0 must
/// change the hash 2,000 ticks later. If it didn't, the hash would be blind to
/// exactly the drift it exists to catch.
#[test]
fn one_ulp_at_tick_zero_changes_the_hash() {
    let a = run(2_000).hash();
    let mut w = scenario::golden();
    let e = &mut w.orgs[0].energy;
    *e = f32::from_bits(e.to_bits() + 1);
    for _ in 0..2_000 {
        w.tick();
    }
    assert_ne!(a, w.hash());
}

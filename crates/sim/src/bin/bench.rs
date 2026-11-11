//! Fixed-population benchmark. Usage: bench [population] [grid] [ticks]
//! Defaults: 5000 organisms, 256x256 grid, 600 ticks after 60 warm-up ticks.
//! Pin it for a quieter number: taskset -c 2 target/release/bench
//! (on this laptop, cores 4-5 belong to ControlForge's real-time threads)

use lifeapi_sim::scenario;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let pop: usize = args.next().map(|s| s.parse().expect("population")).unwrap_or(5000);
    let grid: usize = args.next().map(|s| s.parse().expect("grid")).unwrap_or(256);
    let ticks: u32 = args.next().map(|s| s.parse().expect("ticks")).unwrap_or(600);

    let mut w = scenario::bench(pop, grid);
    for _ in 0..60 {
        w.tick();
    }
    let (mut tg, mut to, mut tc) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let start = Instant::now();
    for _ in 0..ticks {
        let a = Instant::now();
        w.step_grid();
        let b = Instant::now();
        w.step_organisms();
        let c = Instant::now();
        w.step_collisions();
        let d = Instant::now();
        w.tick += 1;
        tg += b - a;
        to += c - b;
        tc += d - c;
    }
    let total = start.elapsed();
    let per = |d: Duration| d.as_secs_f64() * 1e3 / ticks as f64;
    assert_eq!(w.orgs.len(), pop, "fixed population changed");
    println!(
        "population {pop}  grid {grid}x{grid} ({} cells x 2 chemicals)  ticks {ticks}",
        grid * grid
    );
    println!(
        "ticks/s {:.1}   ms/tick {:.3}  (grid {:.3}  organisms {:.3}  collisions {:.3})",
        ticks as f64 / total.as_secs_f64(),
        per(total),
        per(tg),
        per(to),
        per(tc)
    );
}

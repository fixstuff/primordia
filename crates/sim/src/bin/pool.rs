//! Run the golden scenario and print the population every N ticks, so a
//! person can see whether the pool is alive. Usage: pool [ticks] [every]

use primordia_sim::scenario;

fn main() {
    let mut args = std::env::args().skip(1);
    let ticks: u64 = args.next().map(|s| s.parse().expect("ticks")).unwrap_or(10_000);
    let every: u64 = args.next().map(|s| s.parse().expect("every")).unwrap_or(500);
    let mut w = scenario::golden();
    println!("{:>7} {:>6} {:>8} {:>8} {:>10} {:>10} {:>8}", "tick", "pop", "births", "deaths", "sugar", "mineral", "meanE");
    for t in 0..=ticks {
        if t % every == 0 {
            let n = w.orgs.len();
            let mean_e = if n == 0 { 0.0 } else { w.orgs.iter().map(|o| o.energy as f64).sum::<f64>() / n as f64 };
            println!(
                "{:>7} {:>6} {:>8} {:>8} {:>10.0} {:>10.0} {:>8.1}",
                w.tick, n, w.stats.births, w.stats.deaths, w.grid.total(0), w.grid.total(1), mean_e
            );
        }
        if t < ticks {
            w.tick();
        }
    }
    println!("hash {}", w.hash());
}

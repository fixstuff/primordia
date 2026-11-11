//! The objective judge.
//!
//!   lifeapi-eval [--ticks 5000] [--founders 60] [--seeds 1,2,3]
//!                [--together | --round-robin] GENOME.json...
//!
//! Modes, all on a fresh sandbox pool (128x128 cells, 8 sugar and 4 mineral
//! vents) per seed:
//! - solo (default): each genome alone. How well can it live?
//! - --together: every genome released into the same pool at tick 0, the
//!   same number of founders each. A free-for-all.
//! - --round-robin: every pair, one-on-one, on every seed. A pair's winner on
//!   a seed is whichever has more biomass alive at the end.
//!
//! Scores are the population at the end and the biomass (sum of r^2 over the
//! living), averaged over the seeds. Headcount alone rewards swarms of the
//! smallest legal body (seen in the first Opus run: "sitter6"), so the
//! competitive modes rank by biomass. Deterministic: the same inputs always
//! print the same numbers. A genome file may hold one genome object, or
//! {"genome": {...}} as the API takes it.

use lifeapi_sim::genome::Genome;
use lifeapi_sim::scenario;
use lifeapi_sim::World;
use serde_json::{json, Value};

#[derive(PartialEq)]
enum Mode {
    Solo,
    Together,
    RoundRobin,
}

struct Entry {
    file: String,
    genome: Genome,
}

/// Release each genome as its own lineage (in the given order), run, and
/// return (population, biomass) per entry.
fn run(seed: u64, ticks: u64, founders: usize, gs: &[&Genome]) -> Vec<(u64, f64)> {
    let mut w: World = scenario::pool(scenario::base_config(seed, 128, 128), 8, 4);
    let mut lineages = Vec::new();
    for g in gs {
        let sp = w.add_species((*g).clone()).expect("expressed when loaded");
        lineages.push(w.release(sp, founders).expect("species exists"));
    }
    for _ in 0..ticks {
        w.tick();
        if w.orgs.is_empty() {
            break;
        }
    }
    lineages
        .iter()
        .map(|&l| {
            let mut n = 0u64;
            let mut bio = 0f64;
            for o in w.orgs.iter().filter(|o| o.lineage == l) {
                let r = w.species[o.species as usize].phen.radius as f64;
                n += 1;
                bio += r * r;
            }
            (n, bio)
        })
        .collect()
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn main() {
    let (mut ticks, mut founders, mut seeds) = (5000u64, 60usize, vec![1u64, 2, 3]);
    let mut mode = Mode::Solo;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--ticks" => ticks = args.next().expect("--ticks N").parse().expect("number"),
            "--founders" => founders = args.next().expect("--founders N").parse().expect("number"),
            "--seeds" => {
                seeds = args.next().expect("--seeds 1,2,3").split(',').map(|s| s.parse().expect("seed")).collect()
            }
            "--together" => mode = Mode::Together,
            "--round-robin" => mode = Mode::RoundRobin,
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        eprintln!(
            "usage: lifeapi-eval [--ticks N] [--founders N] [--seeds 1,2,3] [--together | --round-robin] GENOME.json..."
        );
        std::process::exit(2);
    }

    // Load; a file that doesn't parse or express is reported, not judged.
    let mut entries = Vec::new();
    let mut rejected = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{f}: {e}"));
        let v = if v.get("genome").is_some() { v["genome"].clone() } else { v };
        match serde_json::from_value::<Genome>(v) {
            Err(e) => rejected.push(json!({"file": f, "error": e.to_string()})),
            Ok(g) => match g.express() {
                Err(e) => rejected.push(json!({"file": f, "name": g.name, "error": e})),
                Ok(_) => entries.push(Entry { file: f.clone(), genome: g }),
            },
        }
    }

    let header = json!({"ticks": ticks, "founders": founders, "seeds": seeds, "rejected": rejected});
    let out = match mode {
        Mode::Solo => {
            let rows: Vec<Value> = entries
                .iter()
                .map(|e| {
                    let per: Vec<(u64, f64)> = seeds.iter().map(|&s| run(s, ticks, founders, &[&e.genome])[0]).collect();
                    let pops: Vec<u64> = per.iter().map(|p| p.0).collect();
                    json!({
                        "file": e.file, "name": e.genome.name,
                        "mean_population": mean(&per.iter().map(|p| p.0 as f64).collect::<Vec<_>>()),
                        "mean_biomass": mean(&per.iter().map(|p| p.1).collect::<Vec<_>>()),
                        "per_seed": pops, "extinct": pops.iter().filter(|&&n| n == 0).count(),
                    })
                })
                .collect();
            json!({"mode": "solo", "results": rows})
        }
        Mode::Together => {
            let gs: Vec<&Genome> = entries.iter().map(|e| &e.genome).collect();
            let per_seed: Vec<Vec<(u64, f64)>> = seeds.iter().map(|&s| run(s, ticks, founders, &gs)).collect();
            let mut rows: Vec<Value> = entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let pops: Vec<f64> = per_seed.iter().map(|r| r[i].0 as f64).collect();
                    let bio: Vec<f64> = per_seed.iter().map(|r| r[i].1).collect();
                    json!({"file": e.file, "name": e.genome.name, "mean_population": mean(&pops),
                           "mean_biomass": mean(&bio), "per_seed_biomass": bio})
                })
                .collect();
            rows.sort_by(|a, b| b["mean_biomass"].as_f64().partial_cmp(&a["mean_biomass"].as_f64()).unwrap());
            json!({"mode": "together", "note": "all released at tick 0 into one pool, ranked by biomass", "results": rows})
        }
        Mode::RoundRobin => {
            let n = entries.len();
            let mut wins = vec![0u32; n];
            let mut games = vec![0u32; n];
            let mut bio_sum = vec![0f64; n];
            let mut matches = Vec::new();
            for i in 0..n {
                for j in i + 1..n {
                    let (mut wi, mut wj) = (0, 0);
                    for &s in &seeds {
                        // Alternate who is released first, so release order can't decide a pair.
                        let r = if s % 2 == 0 {
                            run(s, ticks, founders, &[&entries[i].genome, &entries[j].genome])
                        } else {
                            let r = run(s, ticks, founders, &[&entries[j].genome, &entries[i].genome]);
                            vec![r[1], r[0]]
                        };
                        bio_sum[i] += r[0].1;
                        bio_sum[j] += r[1].1;
                        games[i] += 1;
                        games[j] += 1;
                        if r[0].1 > r[1].1 {
                            wi += 1;
                        } else if r[1].1 > r[0].1 {
                            wj += 1;
                        }
                    }
                    wins[i] += wi;
                    wins[j] += wj;
                    matches.push(json!({"a": entries[i].genome.name, "b": entries[j].genome.name,
                                        "a_wins": wi, "b_wins": wj, "seeds": seeds.len()}));
                }
            }
            let mut table: Vec<Value> = (0..n)
                .map(|i| {
                    json!({"name": entries[i].genome.name, "file": entries[i].file, "wins": wins[i],
                           "games": games[i], "mean_biomass_in_duels": bio_sum[i] / std::cmp::max(games[i], 1) as f64})
                })
                .collect();
            table.sort_by(|a, b| {
                b["wins"].as_u64().cmp(&a["wins"].as_u64()).then(
                    b["mean_biomass_in_duels"].as_f64().partial_cmp(&a["mean_biomass_in_duels"].as_f64()).unwrap(),
                )
            });
            json!({"mode": "round-robin", "note": "a win = more biomass alive at the end of a seed", "table": table, "matches": matches})
        }
    };
    let mut full = header;
    for (k, v) in out.as_object().unwrap() {
        full[k] = v.clone();
    }
    println!("{}", serde_json::to_string_pretty(&full).unwrap());
}

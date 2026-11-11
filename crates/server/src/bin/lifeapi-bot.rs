//! lifeapi-bot [--addr 127.0.0.1:4343] [--ticks 10000] [--chunk 500] [--pace-ms 0]
//! lifeapi-bot --arena [--addr ...] [--name scripted-bot]
//! Founds a lineage on a running server. In a sandbox it steps to the target
//! tick; with --arena it plays the match turn by turn to the end.
//! To watch a sandbox run in the viewer: --chunk 5 --pace-ms 50.

use std::time::Duration;

fn main() {
    let mut addr = "127.0.0.1:4343".to_string();
    let mut name = "scripted-bot".to_string();
    let (mut ticks, mut chunk, mut pace, mut arena) = (10_000u64, 500u64, 0u64, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--arena" {
            arena = true;
            continue;
        }
        let v = args.next().unwrap_or_else(|| panic!("{a} needs a value"));
        match a.as_str() {
            "--addr" => addr = v,
            "--name" => name = v,
            "--ticks" => ticks = v.parse().expect("--ticks is a number"),
            "--chunk" => chunk = v.parse().expect("--chunk is a number"),
            "--pace-ms" => pace = v.parse().expect("--pace-ms is a number"),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    if arena {
        match lifeapi_server::bot::run_arena(&addr, &name, true) {
            Ok(s) => println!("final standings: {s}"),
            Err(e) => {
                eprintln!("lifeapi-bot: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    match lifeapi_server::bot::run_paced(&addr, ticks, chunk, Duration::from_millis(pace), true) {
        Ok(r) => {
            println!(
                "lineage {} at tick {}: {} alive, {} reseeds -> {}",
                r.lineage,
                r.final_tick,
                r.alive,
                r.reseeds,
                if r.alive > 0 { "SURVIVED" } else { "EXTINCT" }
            );
            std::process::exit(if r.alive > 0 { 0 } else { 1 });
        }
        Err(e) => {
            eprintln!("lifeapi-bot: {e}");
            std::process::exit(1);
        }
    }
}

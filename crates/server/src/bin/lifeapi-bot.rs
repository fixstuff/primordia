//! lifeapi-bot [--addr 127.0.0.1:4343] [--ticks 10000] [--chunk 500]
//! Founds a lineage on a running server and steps it to the target tick.

fn main() {
    let mut addr = "127.0.0.1:4343".to_string();
    let (mut ticks, mut chunk) = (10_000u64, 500u64);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().unwrap_or_else(|| panic!("{a} needs a value"));
        match a.as_str() {
            "--addr" => addr = v,
            "--ticks" => ticks = v.parse().expect("--ticks is a number"),
            "--chunk" => chunk = v.parse().expect("--chunk is a number"),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    match lifeapi_server::bot::run(&addr, ticks, chunk, true) {
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

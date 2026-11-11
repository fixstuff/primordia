//! lifeapi-server [--addr 127.0.0.1:4343] [--seed 42]
//! A sandbox pool (128x128 cells, 8 sugar and 4 mineral vents), empty until a
//! player releases something. Time moves only on `step`.

use lifeapi_server::{bind, serve, Game};
use lifeapi_sim::scenario;

fn main() {
    let mut addr = "127.0.0.1:4343".to_string();
    let mut seed = 42u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().expect("--addr needs a value"),
            "--seed" => seed = args.next().expect("--seed needs a value").parse().expect("--seed is a number"),
            other => {
                eprintln!("unknown argument {other}; usage: lifeapi-server [--addr HOST:PORT] [--seed N]");
                std::process::exit(2);
            }
        }
    }
    let listener = match bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lifeapi-server: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("lifeapi-server: sandbox seed {seed}, listening on {}", listener.local_addr().unwrap());
    serve(listener, Game::new(scenario::pool(scenario::base_config(seed, 128, 128), 8, 4)));
}

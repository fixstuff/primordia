//! lifeapi-server [--addr 127.0.0.1:4343] [--http 127.0.0.1:4380 | --no-viewer] [--seed 42]
//! A sandbox pool (128x128 cells, 8 sugar and 4 mineral vents), empty until a
//! player releases something. Time moves only on `step`. The viewer is at
//! http://<--http>/; watch from another machine through an SSH tunnel.

use lifeapi_server::{bind, serve_with_viewer, Game};
use lifeapi_sim::scenario;

fn main() {
    let mut addr = "127.0.0.1:4343".to_string();
    let mut http = Some("127.0.0.1:4380".to_string());
    let mut seed = 42u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().expect("--addr needs a value"),
            "--http" => http = Some(args.next().expect("--http needs a value")),
            "--no-viewer" => http = None,
            "--seed" => seed = args.next().expect("--seed needs a value").parse().expect("--seed is a number"),
            other => {
                eprintln!(
                    "unknown argument {other}; usage: lifeapi-server [--addr HOST:PORT] \
                     [--http HOST:PORT | --no-viewer] [--seed N]"
                );
                std::process::exit(2);
            }
        }
    }
    let open = |a: &str| match bind(a) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lifeapi-server: {a}: {e}");
            std::process::exit(1);
        }
    };
    let api = open(&addr);
    let viewer = http.as_deref().map(open);
    eprintln!("lifeapi-server: sandbox seed {seed}, API on {}", api.local_addr().unwrap());
    if let Some(v) = &viewer {
        eprintln!("lifeapi-server: viewer on http://{}/", v.local_addr().unwrap());
    }
    serve_with_viewer(api, viewer, Game::new(scenario::pool(scenario::base_config(seed, 128, 128), 8, 4)));
}

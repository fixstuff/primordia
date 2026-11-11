//! primordia-server [--addr 127.0.0.1:4343] [--http 127.0.0.1:4380 | --no-viewer]
//!                [--seed 42] [--world NAME|FILE]
//!                [--arena PLAYERS [--window-ticks 500] [--window-secs 120]
//!                 [--match-ticks 10000] [--log FILE] [--results FILE]]
//!
//! Without --arena: a sandbox where time moves only on `step`.
//! With --arena: a match for PLAYERS that starts when they have all said
//! hello; the server's clock moves time in windows (see arena.rs).
//! Worlds: calm, drift, seasons, patchy, desert, bloom, random, or a
//! WorldSpec JSON file. The viewer is at http://<--http>/; watch from
//! another machine through an SSH tunnel.

use primordia_server::arena::ArenaConfig;
use primordia_server::{bind, serve_with_viewer, Game};
use primordia_sim::worlds;
use std::time::Duration;

fn main() {
    let mut addr = "127.0.0.1:4343".to_string();
    let mut http = Some("127.0.0.1:4380".to_string());
    let mut seed = 42u64;
    let mut world = "calm".to_string();
    let mut arena: Option<ArenaConfig> = None;
    let (mut window_ticks, mut window_secs, mut match_ticks) = (500u64, 120f64, 10_000u64);
    let (mut log, mut results) = (None, None);
    let mut first_window_secs: Option<f64> = None;
    let mut combat: Option<String> = None;
    let mut evolve: f32 = 0.0;
    let mut realtime = false;
    let mut args = std::env::args().skip(1);
    let usage = "usage: primordia-server [--addr HOST:PORT] [--http HOST:PORT | --no-viewer] [--seed N] \
                 [--world NAME|FILE] [--combat off|colony|individual] [--evolve-bodies RATE] [--arena PLAYERS [--window-ticks N] [--window-secs S] [--first-window-secs S] [--realtime] [--match-ticks N] \
                 [--log FILE] [--results FILE]]";
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| panic!("{a} needs a value"));
        match a.as_str() {
            "--addr" => addr = val(),
            "--http" => http = Some(val()),
            "--no-viewer" => http = None,
            "--seed" => seed = val().parse().expect("--seed is a number"),
            "--world" => world = val(),
            "--combat" => combat = Some(val()),
            "--evolve-bodies" => evolve = val().parse().expect("--evolve-bodies RATE (0..=1)"),
            "--arena" => {
                arena = Some(ArenaConfig {
                    players: val().parse().expect("--arena PLAYERS"),
                    window_ticks: 0,
                    window: Duration::ZERO,
                    first_window: None,
                    realtime: false,
                    match_ticks: 0,
                    log: None,
                    results: None,
                })
            }
            "--window-ticks" => window_ticks = val().parse().expect("number"),
            "--window-secs" => window_secs = val().parse().expect("number"),
            "--realtime" => realtime = true,
            "--first-window-secs" => first_window_secs = Some(val().parse::<f64>().expect("number")),
            "--match-ticks" => match_ticks = val().parse().expect("number"),
            "--log" => log = Some(val().into()),
            "--results" => results = Some(val().into()),
            other => {
                eprintln!("unknown argument {other}; {usage}");
                std::process::exit(2);
            }
        }
    }
    let spec = worlds::load(&world, seed).unwrap_or_else(|e| {
        eprintln!("primordia-server: {e}");
        std::process::exit(2);
    });
    let mut w = spec.build(seed).unwrap_or_else(|e| {
        eprintln!("primordia-server: {e}");
        std::process::exit(2);
    });
    if let Some(c) = combat.as_deref() {
        w.cfg.combat = primordia_sim::world::CombatMode::parse(c).unwrap_or_else(|e| {
            eprintln!("primordia-server: {e}");
            std::process::exit(2);
        });
    }
    w.cfg.body_mutation = evolve;
    eprintln!("primordia-server: combat {}, body evolution {}", w.cfg.combat.name(), evolve);
    let open = |a: &str| match bind(a) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("primordia-server: {a}: {e}");
            std::process::exit(1);
        }
    };
    let api = open(&addr);
    let viewer = http.as_deref().map(open);
    let mut game = match arena {
        Some(mut cfg) => {
            cfg.window_ticks = window_ticks;
            cfg.window = Duration::from_secs_f64(window_secs);
            cfg.first_window = first_window_secs.map(Duration::from_secs_f64);
            cfg.realtime = realtime;
            cfg.match_ticks = match_ticks;
            cfg.log = log;
            cfg.results = results;
            eprintln!(
                "primordia-server: ARENA for {} players, {} ticks in windows of {} ticks / {}s",
                cfg.players, match_ticks, window_ticks, window_secs
            );
            Game::with_arena(w, cfg)
        }
        None => Game::new(w),
    };
    game.set_world_info(serde_json::to_value(&spec).unwrap());
    eprintln!("primordia-server: world {} seed {seed}, API on {}", spec.name, api.local_addr().unwrap());
    if let Some(v) = &viewer {
        eprintln!("primordia-server: viewer on http://{}/", v.local_addr().unwrap());
    }
    serve_with_viewer(api, viewer, game);
}

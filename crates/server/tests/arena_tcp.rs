//! An arena match end to end over real TCP: three scripted bots join a
//! server whose clock owns time; the match starts when the third joins,
//! runs its windows, and every bot receives the same final standings.

use lifeapi_server::arena::ArenaConfig;
use lifeapi_server::{bind, bot, serve, Game};
use lifeapi_sim::worlds;
use std::thread;
use std::time::Duration;

#[test]
fn three_bots_play_a_match_to_the_end() {
    let l = bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let dir = std::env::temp_dir().join(format!("lifeapi-arena-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (log, results) = (dir.join("log.jsonl"), dir.join("results.json"));
    let w = worlds::preset("drift", 3).unwrap().build(3).unwrap();
    let cfg = ArenaConfig {
        players: 3,
        window_ticks: 250,
        window: Duration::from_secs(20),
        first_window: None,
        match_ticks: 1500,
        log: Some(log.clone()),
        results: Some(results.clone()),
    };
    thread::spawn(move || serve(l, Game::with_arena(w, cfg)));

    let bots: Vec<_> = (0..3)
        .map(|i| {
            let addr = addr.clone();
            thread::spawn(move || bot::run_arena(&addr, &format!("bot{i}"), false))
        })
        .collect();
    let finals: Vec<_> = bots.into_iter().map(|b| b.join().unwrap().expect("bot played")).collect();
    assert_eq!(finals[0], finals[1]);
    assert_eq!(finals[1], finals[2]);
    assert_eq!(finals[0].as_array().unwrap().len(), 3);
    // Windows closed because everyone ended their turn, not by timeout:
    // 6 windows of 250 ticks in far less than 6 x 20 s.
    let r: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&results).unwrap()).unwrap();
    assert_eq!(r["tick"], 1500);
    let log = std::fs::read_to_string(&log).unwrap();
    assert_eq!(log.lines().filter(|l| l.contains("\"advance\"")).count(), 6);
    assert!(log.contains("\"start\"") && log.contains("\"over\""));
    // Every join is logged with its player index, so the log maps index -> name.
    assert_eq!(log.lines().filter(|l| l.contains("hello")).count(), 3, "{log}");
    std::fs::remove_dir_all(&dir).ok();
}

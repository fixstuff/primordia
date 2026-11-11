//! M1's done-condition, end to end over real TCP: the scripted bot founds a
//! lineage on a fresh sandbox server and it is still alive at tick 10 000.
//! The bot may reseed from its budget; the test also requires the lineage
//! to have sustained itself (not be living on reseeds alone).

use lifeapi_server::{bind, bot, serve, Game};
use lifeapi_sim::scenario;
use std::thread;

#[test]
fn scripted_bot_lineage_survives_10000_ticks() {
    let listener = bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let game = Game::new(scenario::pool(scenario::base_config(42, 128, 128), 8, 4));
    thread::spawn(move || serve(listener, game));

    let r = bot::run(&addr, 10_000, 500, false).expect("bot ran");
    eprintln!("{r:?}");
    assert_eq!(r.final_tick, 10_000);
    assert!(r.alive > 0, "lineage extinct: {r:?}");
    assert!(r.reseeds <= 2, "lineage only alive by reseeding: {r:?}");
    let peak = r.curve.iter().map(|&(_, a)| a).max().unwrap();
    assert!(peak > 60, "the lineage never grew past its 60 founders: {r:?}");
}

//! Arena matches: the server owns the clock.
//!
//! A match waits until `players` have said hello, then runs in windows. In
//! each window players act (submit, release, report, ...) and call
//! `end_turn`. The window closes when every player has ended their turn or
//! its wall-clock time runs out, whichever comes first; then the world
//! advances `window_ticks` and every waiting `end_turn` gets its reply with
//! the scoreboard. A slow player misses that window's moves; it never stalls
//! the match. `step` and `subscribe` are refused: time is not the players'
//! to move.
//!
//! The score is a player's biomass (sum of r^2 over its living organisms)
//! averaged over every tick of the match, stated in `catalog` and every
//! reply. (Match 1 scored the final tick only; deadline dumps of
//! short-lived giants decided it.)
//!
//! Every request line, with its player and tick, and every window advance
//! goes to the match log, in the order applied. Replaying the log against
//! the same world and seed reproduces the match (no replay tool yet).

use crate::{Game, Request};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ArenaConfig {
    pub players: usize,
    pub window_ticks: u64,
    pub window: Duration,
    /// The first window's limit, if longer: every player reads the catalog
    /// and designs from nothing then (League 1: Fable missed window 1 in all
    /// six matches, DeepSeek in five, at 180 s). None = `window`.
    pub first_window: Option<Duration>,
    /// Continuous time: the world advances steadily through each window on
    /// the server's clock (window_ticks per window), never waiting for
    /// anyone. Moves land at the tick they arrive; end_turn just waits for
    /// the window's end. The first window is a design phase with no ticks;
    /// it ends early once everyone has ended their turn.
    pub realtime: bool,
    /// How long after the first player joins to wait for the rest. Then the
    /// match starts with whoever is here, and anyone missing simply scores
    /// nothing (League 5: a player whose API had no credit never joined, and
    /// six matches sat at tick 0 waiting for it). None = wait for everyone.
    pub join_wait: Option<Duration>,
    pub match_ticks: u64,
    pub log: Option<PathBuf>,
    pub results: Option<PathBuf>,
}

pub struct Arena {
    pub cfg: ArenaConfig,
    /// Per player: biomass summed over every tick played. The score is this
    /// divided by match_ticks, so every tick counts the same and a deadline
    /// dump of short-lived bodies counts for little (arena match 1).
    biomass_ticks: Vec<f64>,
    started: bool,
    /// When the match starts anyway if players are still missing.
    join_deadline: Option<Instant>,
    /// Players the match is played by: cfg.players, or fewer if it started
    /// short. Only these are waited for to end the design window early.
    expected: usize,
    /// Filled when the match starts: answer every end_turn made while waiting
    /// for players, so the design window opens for everyone at the start
    /// (League 5: end_turns made during the join wait counted, the design
    /// window closed at once, and founders landed at ticks 4-78, not 0).
    answer_now: Vec<usize>,
    window: u64,
    deadline: Option<Instant>,
    /// Realtime: when the current window opened, and ticks run in it so far.
    window_start: Option<Instant>,
    window_done: u64,
    ended: Vec<bool>,
    waiting: Vec<usize>,
    over: bool,
    log: Option<std::fs::File>,
}

impl Arena {
    fn new(cfg: ArenaConfig) -> Arena {
        let log = cfg.log.as_ref().map(|p| std::fs::File::create(p).expect("arena log file"));
        let expected = cfg.players;
        Arena { cfg, biomass_ticks: Vec::new(), started: false, join_deadline: None, expected, answer_now: Vec::new(),
                window: 0, deadline: None, ended: Vec::new(),
                waiting: Vec::new(), over: false, log,
                window_start: None, window_done: 0 }
    }

    pub fn rules(&self) -> Value {
        json!({
            "score": "your biomass (sum of radius^2 over your living organisms BORN IN THE POOL; founders you release \
                      do not count, only their descendants) averaged over every tick of the match. Every tick counts \
                      the same, and only lineages that eat and reproduce score.",
            "founder_energy": "a founder starts with r^2 energy (half its body's 2r^2); vacuoles start empty",
            "founder_cost": "5 * (radius/5)^2 budget per founder: radius 2 = 0.8, 5 = 5, 10 = 20, 20 = 80",
            "players": self.cfg.players,
            "window_ticks": self.cfg.window_ticks,
            "window_seconds": self.cfg.window.as_secs_f64(),
            "clock": if self.cfg.realtime {
                "continuous: after the first (design) window the world runs on its own, window_ticks per \
                 window_seconds, and never waits. Your moves land at the tick they arrive. end_turn waits \
                 for the window's end and returns the scoreboard."
            } else {
                "turns: the world advances window_ticks when everyone has ended their turn or the window times out"
            },
            "first_window_seconds": self.cfg.first_window.unwrap_or(self.cfg.window).as_secs_f64(),
            "match_ticks": self.cfg.match_ticks,
            "how": "act (catalog, report, submit_genome, release, observe), then call end_turn; \
                    it returns when the next window opens, with the scoreboard. step and subscribe are refused.",
        })
    }

    pub(crate) fn log_line(&mut self, player: usize, tick: u64, line: &str) {
        let w = self.window;
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(f, "{}", json!({"window": w, "tick": tick, "player": player, "line": line}));
        }
    }

    /// Start the match with `n` players joined (fewer than cfg.players if the
    /// join wait ran out).
    fn start(&mut self, n: usize, now: Instant) {
        self.started = true;
        self.expected = n;
        // Turns ended before the start don't end the design window.
        self.ended.iter_mut().for_each(|e| *e = false);
        self.answer_now = std::mem::take(&mut self.waiting);
        self.deadline = Some(now + self.cfg.first_window.unwrap_or(self.cfg.window));
        self.window_start = Some(now);
        let missing = self.cfg.players.saturating_sub(n);
        self.log_event(json!({"event": "start", "tick": 0, "players": n, "missing": missing}));
    }

    fn log_event(&mut self, ev: Value) {
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(f, "{ev}");
        }
    }
}

impl Game {
    /// A game that runs as an arena match.
    pub fn with_arena(world: primordia_sim::World, cfg: ArenaConfig) -> Game {
        let mut g = Game::new(world);
        g.arena = Some(Arena::new(cfg));
        g
    }

    /// The match as the viewer needs it: how long, whether it has started or
    /// ended. None outside an arena.
    pub fn match_info(&self) -> Option<Value> {
        let a = self.arena.as_ref()?;
        Some(json!({"match_ticks": a.cfg.match_ticks, "started": a.started, "over": a.over,
                    "window": a.window, "realtime": a.cfg.realtime,
                    "joined": self.players.len(), "seats": a.cfg.players}))
    }

    /// When the current window closes, if a match is running.
    pub fn arena_deadline(&self) -> Option<Instant> {
        self.arena.as_ref().and_then(|a| {
            if a.over {
                None
            } else if !a.started {
                a.join_deadline
            } else if a.cfg.realtime && a.started && a.window > 0 {
                // Wake ~20 times a second so the world (and the viewer) moves smoothly.
                a.deadline.map(|d| d.min(Instant::now() + Duration::from_millis(50)))
            } else {
                a.deadline
            }
        })
    }

    /// Called on hello: start the match once everyone is here.
    pub(crate) fn arena_joined(&mut self) -> Option<Value> {
        let n = self.players.len();
        let a = self.arena.as_mut()?;
        a.ended.resize(n, false);
        let now = Instant::now();
        if !a.started && n >= a.cfg.players {
            a.start(n, now);
        } else if !a.started && a.join_deadline.is_none() {
            a.join_deadline = a.cfg.join_wait.map(|w| now + w);
        }
        Some(json!({"started": a.started, "joined": n, "rules": a.rules()}))
    }

    pub(crate) fn arena_refuses(&self, req: &Request) -> Option<String> {
        let a = self.arena.as_ref()?;
        match req {
            Request::Step { .. } => Some("arena: the server's clock owns time. Make your moves, then call end_turn.".into()),
            Request::Subscribe { .. } => Some("arena: no subscriptions; end_turn returns the scoreboard every window.".into()),
            Request::Release { .. } | Request::SubmitGenome { .. } if a.over => Some("arena: the match is over.".into()),
            Request::Release { .. } if !a.started => Some(format!(
                "arena: waiting for players ({} of {} joined); release once the match starts. You may submit genomes now.",
                self.players.len(),
                a.cfg.players
            )),
            _ => None,
        }
    }

    /// Mark the player done for this window. None = reply deferred.
    pub(crate) fn arena_end_turn(&mut self, p: usize) -> Option<String> {
        let over = self.arena.as_ref().is_some_and(|a| a.over);
        if over {
            return Some(self.turn_reply(p));
        }
        let a = self.arena.as_mut().unwrap();
        if p >= a.ended.len() {
            a.ended.resize(p + 1, false);
        }
        a.ended[p] = true;
        if !a.waiting.contains(&p) {
            a.waiting.push(p);
        }
        None
    }

    /// Close the window if it's time; returns the deferred replies to send.
    pub fn arena_poll(&mut self, now: Instant) -> Vec<(usize, String)> {
        let n = self.players.len();
        let Some(a) = self.arena.as_mut() else { return Vec::new() };
        if !a.started && n > 0 && a.join_deadline.is_some_and(|d| now >= d) {
            // The join wait is over: play with whoever came.
            a.start(n, now);
        }
        if !a.answer_now.is_empty() {
            let early = std::mem::take(&mut a.answer_now);
            return early.into_iter().map(|p| (p, self.turn_reply(p))).collect();
        }
        if !a.started || a.over {
            return Vec::new();
        }
        let everyone_done = a.ended.len() >= a.expected && a.ended.iter().all(|&e| e);
        let timed_out = a.deadline.is_some_and(|d| now >= d);
        let realtime = a.cfg.realtime;
        // Realtime design window: no ticks, ends on time or when everyone is ready.
        let design = realtime && a.window == 0;
        if realtime && !design {
            // Run the world up to where the clock says this window should be.
            let (start, len) = (a.window_start.unwrap_or(now), a.cfg.window.as_secs_f64().max(1e-9));
            let frac = (now.saturating_duration_since(start).as_secs_f64() / len).min(1.0);
            let due = (a.cfg.window_ticks as f64 * frac) as u64;
            let n = due.saturating_sub(a.window_done);
            self.advance(n);
            if !timed_out {
                return Vec::new();
            }
        } else if !(everyone_done || timed_out) {
            return Vec::new();
        }
        let a = self.arena.as_ref().unwrap();
        let missed: Vec<usize> = a.ended.iter().enumerate().filter(|(_, &e)| !e).map(|(i, _)| i).collect();
        // Turns: the whole window now. Realtime: whatever the window still owes.
        let n = if design { 0 } else { a.cfg.window_ticks.saturating_sub(if realtime { a.window_done } else { 0 }) };
        self.advance(n);
        let tick = self.world.tick;
        let standings = self.standings();
        let a = self.arena.as_mut().unwrap();
        a.window += 1;
        a.ended.iter_mut().for_each(|e| *e = false);
        a.deadline = Some(now + a.cfg.window);
        a.window_start = Some(now);
        a.window_done = 0;
        let over = tick >= a.cfg.match_ticks;
        a.over = over;
        let w = a.window;
        a.log_event(json!({"event": "advance", "window": w, "tick": tick, "missed": missed, "standings": standings}));
        if over {
            a.log_event(json!({"event": "over", "tick": tick, "standings": standings}));
            if let Some(path) = a.cfg.results.clone() {
                let _ = std::fs::write(
                    path,
                    serde_json::to_string_pretty(&json!({"tick": tick, "rules": a.rules(), "standings": standings,
                                                         "world": self.world_info}))
                        .unwrap(),
                );
            }
        }
        let waiting = std::mem::take(&mut self.arena.as_mut().unwrap().waiting);
        waiting.into_iter().map(|p| (p, self.turn_reply(p))).collect()
    }

    /// Run the world `n` ticks (capped at the match's end), scoring every tick.
    fn advance(&mut self, n: u64) {
        let n = n.min(self.arena.as_ref().map_or(0, |a| a.cfg.match_ticks.saturating_sub(self.world.tick)));
        if n == 0 {
            return;
        }
        for _ in 0..n {
            self.world.tick();
            if self.world.tick % self.hist_every == 0 {
                self.sample_history();
            }
            let bio = self.player_biomass();
            let a = self.arena.as_mut().unwrap();
            a.biomass_ticks.resize(bio.len(), 0.0);
            for (s, b) in a.biomass_ticks.iter_mut().zip(bio) {
                *s += b;
            }
        }
        self.arena.as_mut().unwrap().window_done += n;
        for pl in self.players.iter_mut() {
            pl.budget = (pl.budget + crate::BUDGET_PER_TICK * n as f64).min(crate::BUDGET_CAP);
        }
        self.version += 1;
    }

    /// Scoring biomass per player right now: sum of r^2 over living organisms
    /// BORN IN THE POOL (generation >= 1). Founders don't count. Match 1 was
    /// won by mouthless giants and match 2 by non-feeding "batteries"; both
    /// were founders that never reproduced, and no founder price closes that
    /// gap. Only reproduction does.
    fn player_biomass(&self) -> Vec<f64> {
        let mut bio = vec![0.0; self.players.len()];
        for o in self.world.orgs.iter().filter(|o| o.generation >= 1) {
            if let Some(p) = self.lineage_owner.get(o.lineage as usize).copied().flatten() {
                let r = self.world.species[o.species as usize].phen.radius as f64;
                bio[p] += r * r;
            }
        }
        bio
    }

    /// Players ranked by score (biomass averaged over the match so far,
    /// out of the whole match), then by biomass now.
    pub fn standings(&self) -> Vec<Value> {
        let bio = self.player_biomass();
        let mut alive = vec![0u64; self.players.len()];
        for o in &self.world.orgs {
            if let Some(p) = self.lineage_owner.get(o.lineage as usize).copied().flatten() {
                alive[p] += 1;
            }
        }
        let (sums, match_ticks) = match self.arena.as_ref() {
            Some(a) => (a.biomass_ticks.clone(), a.cfg.match_ticks.max(1) as f64),
            None => (Vec::new(), 1.0),
        };
        let mut rows: Vec<(usize, u64, f64, f64)> = (0..self.players.len())
            .map(|p| (p, alive[p], bio[p], sums.get(p).copied().unwrap_or(0.0) / match_ticks))
            .collect();
        rows.sort_by(|a, b| {
            b.3.partial_cmp(&a.3).unwrap().then(b.2.partial_cmp(&a.2).unwrap()).then(a.0.cmp(&b.0))
        });
        rows.iter()
            .enumerate()
            .map(|(rank, &(p, n, bio, score))| {
                json!({"rank": rank + 1, "player": p, "name": self.players[p].name, "alive": n,
                       "biomass": bio, "score": score})
            })
            .collect()
    }

    fn turn_reply(&self, p: usize) -> String {
        let a = self.arena.as_ref().unwrap();
        let standings = self.standings();
        let me = standings.iter().find(|s| s["player"] == p).cloned();
        self.ok(json!({
            "window": a.window,
            "match_over": a.over,
            "ticks_left": a.cfg.match_ticks.saturating_sub(self.world.tick),
            "window_seconds": a.cfg.window.as_secs_f64(),
            "you": me,
            "standings": standings,
            "budget": self.players[p].budget,
            "hint": if a.over { "The match is over." } else { "Call report to see what happened to your lineages, act, then end_turn." },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primordia_sim::scenario;

    fn arena(players: usize, window_ms: u64) -> Game {
        Game::with_arena(
            scenario::pool(scenario::base_config(1, 64, 64), 4, 2),
            ArenaConfig { players, window_ticks: 100, window: Duration::from_millis(window_ms),
                          first_window: None, realtime: false, join_wait: None, match_ticks: 300, log: None, results: None },
        )
    }

    fn call(g: &mut Game, p: Option<usize>, line: &str) -> (Option<Value>, Option<usize>) {
        let (s, p) = g.handle_deferrable(p, line);
        (s.map(|s| serde_json::from_str(&s).unwrap()), p)
    }

    fn join_and_release(g: &mut Game, name: &str) -> Option<usize> {
        let (_, p) = call(g, None, &format!(r#"{{"op":"hello","name":"{name}"}}"#));
        let (r, _) = call(g, p, &format!(r#"{{"op":"submit_genome","genome":{}}}"#, scenario::FOUNDER.replace('\n', "")));
        let sp = r.unwrap()["species"].clone();
        let _ = call(g, p, &format!(r#"{{"op":"release","species":{sp},"count":20}}"#));
        p
    }

    /// Realtime: the design window runs no ticks and ends when everyone is
    /// ready; then the world moves with the clock, part-way through a window
    /// at part of its ticks, and doesn't wait for a player who has ended.
    #[test]
    fn realtime_world_moves_with_the_clock() {
        let mut g = arena(1, 1_000);
        g.arena.as_mut().unwrap().cfg.realtime = true;
        let p = join_and_release(&mut g, "a");
        assert!(call(&mut g, p, r#"{"op":"end_turn"}"#).0.is_none());
        let t0 = Instant::now();
        assert_eq!(g.arena_poll(t0).len(), 1, "design window ends once everyone is ready");
        assert_eq!(g.world.tick, 0, "the design window runs no ticks");
        assert!(g.arena_deadline().unwrap() <= Instant::now() + Duration::from_millis(60), "wakes often");
        // Halfway through window 1: about half its 100 ticks, and no reply yet.
        assert!(g.arena_poll(t0 + Duration::from_millis(500)).is_empty());
        assert!((45..=55).contains(&g.world.tick), "tick {}", g.world.tick);
        // Ending the turn early doesn't close the window or jump the world.
        assert!(call(&mut g, p, r#"{"op":"end_turn"}"#).0.is_none());
        assert!(g.arena_poll(t0 + Duration::from_millis(600)).is_empty());
        assert!(g.world.tick < 100, "tick {}", g.world.tick);
        // At the window's end: exactly 100 ticks, and the waiting reply goes out.
        assert_eq!(g.arena_poll(t0 + Duration::from_millis(1_000)).len(), 1);
        assert_eq!(g.world.tick, 100);
    }

    /// The first window can be longer; later windows use the normal limit.
    #[test]
    fn first_window_is_longer() {
        let mut g = arena(1, 1_000);
        g.arena.as_mut().unwrap().cfg.first_window = Some(Duration::from_secs(600));
        let p = join_and_release(&mut g, "a");
        let left = g.arena_deadline().unwrap() - Instant::now();
        assert!(left > Duration::from_secs(500), "{left:?}");
        assert!(call(&mut g, p, r#"{"op":"end_turn"}"#).0.is_none());
        assert_eq!(g.arena_poll(Instant::now()).len(), 1);
        let left = g.arena_deadline().unwrap() - Instant::now();
        assert!(left <= Duration::from_secs(1), "{left:?}");
    }

    /// A player that never joins doesn't freeze the match: after the join
    /// wait it starts with whoever came, and plays to the end.
    #[test]
    fn a_no_show_does_not_freeze_the_match() {
        let mut g = arena(3, 60_000);
        g.arena.as_mut().unwrap().cfg.join_wait = Some(Duration::from_millis(200));
        let a = join_and_release(&mut g, "a");
        let b = join_and_release(&mut g, "b");
        let joined = Instant::now();
        assert!(!g.arena.as_ref().unwrap().started, "still waiting for the third player");
        assert!(g.arena_deadline().is_some(), "the join wait wakes the server");
        assert!(g.arena_poll(joined).is_empty());
        assert!(!g.arena.as_ref().unwrap().started, "not before the wait is over");
        g.arena_poll(joined + Duration::from_millis(300));
        assert!(g.arena.as_ref().unwrap().started, "started short-handed");
        // Both present players ending their turn closes the window: the
        // missing one is not waited for.
        for _ in 0..3 {
            for p in [a, b] {
                let _ = call(&mut g, p, r#"{"op":"release","species":0,"count":5}"#);
                assert!(call(&mut g, p, r#"{"op":"end_turn"}"#).0.is_none());
            }
            assert_eq!(g.arena_poll(Instant::now()).len(), 2);
        }
        assert_eq!(g.world.tick, 300);
        assert!(g.arena.as_ref().unwrap().over);
    }

    /// end_turns made before the start are answered when it starts and
    /// don't close the design window: it opens for everyone at the start.
    #[test]
    fn turns_ended_while_waiting_do_not_close_the_design_window() {
        let mut g = arena(2, 60_000);
        g.arena.as_mut().unwrap().cfg.realtime = true;
        let (_, a) = call(&mut g, None, r#"{"op":"hello","name":"a"}"#);
        assert!(call(&mut g, a, r#"{"op":"end_turn"}"#).0.is_none(), "a waits for the start");
        let (_, b) = call(&mut g, None, r#"{"op":"hello","name":"b"}"#);
        let r = g.arena_poll(Instant::now());
        assert_eq!(r.len(), 1, "a is answered when the match starts");
        assert_eq!(r[0].0, a.unwrap());
        assert_eq!(g.arena.as_ref().unwrap().window, 0, "still the design window");
        // The design window closes only when both end their turn now. b goes
        // first: a's end_turn from before the start must not count for it.
        assert!(call(&mut g, b, r#"{"op":"end_turn"}"#).0.is_none());
        assert!(g.arena_poll(Instant::now()).is_empty(), "a hasn't ended its turn since the start");
        assert_eq!(g.arena.as_ref().unwrap().window, 0, "still the design window");
        assert!(call(&mut g, a, r#"{"op":"end_turn"}"#).0.is_none());
        assert_eq!(g.arena_poll(Instant::now()).len(), 2);
        assert_eq!(g.arena.as_ref().unwrap().window, 1);
    }

    /// Without a join wait the arena waits for everyone, as before.
    #[test]
    fn without_a_join_wait_the_arena_waits() {
        let mut g = arena(3, 1);
        join_and_release(&mut g, "a");
        assert!(g.arena_deadline().is_none());
        g.arena_poll(Instant::now() + Duration::from_secs(3600));
        assert!(!g.arena.as_ref().unwrap().started);
    }

    #[test]
    fn step_is_refused_and_the_window_closes_when_everyone_is_done() {
        let mut g = arena(2, 60_000);
        let a = join_and_release(&mut g, "a");
        // Before the second player joins, release is refused: the match hasn't started.
        let (r, _) = call(&mut g, a, r#"{"op":"observe"}"#);
        assert_eq!(r.unwrap()["lineages"].as_array().unwrap().len(), 0);
        let b = join_and_release(&mut g, "b");
        let a = join_and_release(&mut g, "a2").or(a); // the arena is full
        let _ = a;
        let (r, _) = call(&mut g, b, r#"{"op":"step","ticks":10}"#);
        assert!(r.unwrap()["error"].as_str().unwrap().contains("clock owns time"));
        // a re-releases now that the match has started.
        let (r, _) = call(&mut g, Some(0), r#"{"op":"release","species":0,"count":20}"#);
        assert_eq!(r.unwrap()["ok"], true);
        // Both end their turn: no reply until the window closes, then both get one.
        assert!(call(&mut g, Some(0), r#"{"op":"end_turn"}"#).0.is_none());
        assert!(g.arena_poll(Instant::now()).is_empty(), "b hasn't ended its turn yet");
        assert!(call(&mut g, b, r#"{"op":"end_turn"}"#).0.is_none());
        let replies = g.arena_poll(Instant::now());
        assert_eq!(replies.len(), 2);
        let r: Value = serde_json::from_str(&replies[0].1).unwrap();
        assert_eq!(r["tick"], 100);
        assert_eq!(r["window"], 1);
        assert_eq!(r["standings"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_slow_player_misses_the_window_but_never_stalls_it() {
        let mut g = arena(2, 1);
        let a = join_and_release(&mut g, "fast");
        let _slow = join_and_release(&mut g, "slow");
        let _ = call(&mut g, Some(0), r#"{"op":"release","species":0,"count":20}"#);
        assert!(call(&mut g, a, r#"{"op":"end_turn"}"#).0.is_none());
        let later = Instant::now() + Duration::from_millis(5);
        let replies = g.arena_poll(later);
        assert_eq!(replies.len(), 1, "only the fast player was waiting");
        assert_eq!(g.world().tick, 100);
    }

    #[test]
    fn the_match_ends_at_match_ticks_with_standings_and_writes_are_refused() {
        let mut g = arena(1, 1);
        let p = join_and_release(&mut g, "solo");
        for _ in 0..3 {
            assert!(call(&mut g, p, r#"{"op":"end_turn"}"#).0.is_none());
            g.arena_poll(Instant::now());
        }
        assert_eq!(g.world().tick, 300);
        let (r, _) = call(&mut g, p, r#"{"op":"end_turn"}"#);
        let r = r.expect("after the match, end_turn answers at once");
        assert_eq!(r["match_over"], true);
        assert_eq!(r["standings"][0]["name"], "solo");
        let (r, _) = call(&mut g, p, r#"{"op":"release","species":0,"count":1}"#);
        assert!(r.unwrap()["error"].as_str().unwrap().contains("over"));
    }

    /// Match 1's exploit, replayed: giants dumped near the end score a
    /// fraction of what a lineage that held the pool all match scores.
    #[test]
    fn score_is_biomass_averaged_over_the_match() {
        let mut g = arena(2, 1);
        let early = join_and_release(&mut g, "early");
        // "late" joins (starting the match) and submits, but releases nothing yet.
        let (_, late) = call(&mut g, None, r#"{"op":"hello","name":"late"}"#);
        let _ = call(&mut g, late, &format!(r#"{{"op":"submit_genome","genome":{}}}"#, scenario::FOUNDER.replace('\n', "")));
        let _ = call(&mut g, early, r#"{"op":"release","species":0,"count":20}"#);
        for w in 0..3 {
            if w == 2 {
                // The late player's own founders arrive for the last window only.
                let _ = call(&mut g, late, r#"{"op":"release","species":1,"count":40}"#);
            }
            let _ = call(&mut g, early, r#"{"op":"end_turn"}"#);
            let _ = call(&mut g, late, r#"{"op":"end_turn"}"#);
            g.arena_poll(Instant::now());
        }
        let s = g.standings();
        let by = |n: &str| s.iter().find(|r| r["name"] == n).unwrap().clone();
        let (e, l) = (by("early"), by("late"));
        // Score = mean over all 300 ticks: the late player held biomass for ~100.
        assert!(l["score"].as_f64().unwrap() <= 0.5 * l["biomass"].as_f64().unwrap(), "{l}");
        assert!(l["score"].as_f64().unwrap() < e["score"].as_f64().unwrap(), "{l} vs {e}");
        assert_eq!(s[0]["name"], "early", "{s:?}");
        let _ = e;
    }

    /// Founders never score: a player whose bodies never reproduce scores 0,
    /// however many it releases.
    #[test]
    fn only_organisms_born_in_the_pool_score() {
        let mut g = arena(2, 1);
        let breeder = join_and_release(&mut g, "breeder");
        let (_, blobs) = call(&mut g, None, r#"{"op":"hello","name":"blobs"}"#);
        let blob = r#"{"schema":"lifeapi.genome/1","name":"blob","body":[{"part":"membrane","radius":20}],"brain":{"hidden":1},"mutation":{"rate":0,"size":0}}"#;
        let (r, _) = call(&mut g, blobs, &format!(r#"{{"op":"submit_genome","genome":{blob}}}"#));
        let sp = r.unwrap()["species"].clone();
        let (r, _) = call(&mut g, blobs, &format!(r#"{{"op":"release","species":{sp},"count":6}}"#));
        assert_eq!(r.unwrap()["ok"], true);
        let _ = call(&mut g, breeder, r#"{"op":"release","species":0,"count":20}"#);
        for _ in 0..3 {
            let _ = call(&mut g, breeder, r#"{"op":"end_turn"}"#);
            let _ = call(&mut g, blobs, r#"{"op":"end_turn"}"#);
            g.arena_poll(Instant::now());
        }
        let s = g.standings();
        let by = |n: &str| s.iter().find(|r| r["name"] == n).unwrap().clone();
        assert_eq!(by("blobs")["alive"], 6, "the blobs are alive...");
        assert_eq!(by("blobs")["score"], 0.0, "...but founders don't score");
        assert!(by("breeder")["score"].as_f64().unwrap() > 0.0, "{s:?}");
    }

    #[test]
    fn a_dropped_player_rejoins_by_name_but_strangers_cannot_join_a_full_match() {
        let mut g = arena(2, 60_000);
        let _a = join_and_release(&mut g, "alpha");
        let _b = join_and_release(&mut g, "beta");
        let (r, p) = call(&mut g, None, r#"{"op":"hello","name":"beta"}"#);
        let r = r.unwrap();
        assert_eq!(r["rejoined"], true, "{r}");
        assert_eq!(p, Some(1));
        let (r, p) = call(&mut g, None, r#"{"op":"hello","name":"gamma"}"#);
        assert!(r.unwrap()["error"].as_str().unwrap().contains("full"));
        assert_eq!(p, None);
    }

    #[test]
    fn end_turn_outside_an_arena_is_refused() {
        let mut g = Game::new(scenario::pool(scenario::base_config(1, 64, 64), 4, 2));
        let (_, p) = g.handle(None, r#"{"op":"hello","name":"x"}"#);
        let (r, _) = g.handle(p, r#"{"op":"end_turn"}"#);
        assert!(r.contains("use step"), "{r}");
    }
}

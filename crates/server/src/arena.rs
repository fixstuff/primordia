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
    window: u64,
    deadline: Option<Instant>,
    ended: Vec<bool>,
    waiting: Vec<usize>,
    over: bool,
    log: Option<std::fs::File>,
}

impl Arena {
    fn new(cfg: ArenaConfig) -> Arena {
        let log = cfg.log.as_ref().map(|p| std::fs::File::create(p).expect("arena log file"));
        Arena { cfg, biomass_ticks: Vec::new(), started: false, window: 0, deadline: None, ended: Vec::new(),
                waiting: Vec::new(), over: false, log }
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

    fn log_event(&mut self, ev: Value) {
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(f, "{ev}");
        }
    }
}

impl Game {
    /// A game that runs as an arena match.
    pub fn with_arena(world: lifeapi_sim::World, cfg: ArenaConfig) -> Game {
        let mut g = Game::new(world);
        g.arena = Some(Arena::new(cfg));
        g
    }

    /// When the current window closes, if a match is running.
    pub fn arena_deadline(&self) -> Option<Instant> {
        self.arena.as_ref().and_then(|a| if a.over { None } else { a.deadline })
    }

    /// Called on hello: start the match once everyone is here.
    pub(crate) fn arena_joined(&mut self) -> Option<Value> {
        let n = self.players.len();
        let a = self.arena.as_mut()?;
        a.ended.resize(n, false);
        if !a.started && n >= a.cfg.players {
            a.started = true;
            a.deadline = Some(Instant::now() + a.cfg.first_window.unwrap_or(a.cfg.window));
            a.log_event(json!({"event": "start", "tick": 0, "players": n}));
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
        let Some(a) = self.arena.as_ref() else { return Vec::new() };
        if !a.started || a.over {
            return Vec::new();
        }
        let everyone_done = a.ended.len() >= a.cfg.players && a.ended.iter().all(|&e| e);
        let timed_out = a.deadline.is_some_and(|d| now >= d);
        if !(everyone_done || timed_out) {
            return Vec::new();
        }
        let missed: Vec<usize> = a.ended.iter().enumerate().filter(|(_, &e)| !e).map(|(i, _)| i).collect();
        let n = a.cfg.window_ticks.min(a.cfg.match_ticks.saturating_sub(self.world.tick));
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
        for pl in self.players.iter_mut() {
            pl.budget = (pl.budget + crate::BUDGET_PER_TICK * n as f64).min(crate::BUDGET_CAP);
        }
        self.version += 1;
        let tick = self.world.tick;
        let standings = self.standings();
        let a = self.arena.as_mut().unwrap();
        a.window += 1;
        a.ended.iter_mut().for_each(|e| *e = false);
        a.deadline = Some(now + a.cfg.window);
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
    use lifeapi_sim::scenario;

    fn arena(players: usize, window_ms: u64) -> Game {
        Game::with_arena(
            scenario::pool(scenario::base_config(1, 64, 64), 4, 2),
            ArenaConfig { players, window_ticks: 100, window: Duration::from_millis(window_ms),
                          first_window: None, match_ticks: 300, log: None, results: None },
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

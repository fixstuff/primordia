# LifeAPI — Design

> The game is **LifeAPI** (D-5). The project folder is still `SporeAI`; that
> name stays off the game, because "Spore" is EA's trademark.
> Status: DRAFT 9, 2026-11-10. M0–M4 built (§10–§15); two arena matches played. Still owed: the droplet run of the golden hash, and
> M2's watch from a second machine. Draft 1's claims were checked
> by a fresh-context reviewer, and its findings are folded in. Open decisions are marked **D-n**
> and collected at the end. They're yours to rule on.

## 1. The idea in one paragraph

An evolution game in which every species has a mind, and that mind can be an AI.
Players design organisms (body, senses, behaviour) and release them into one
shared, persistent world. The world, not a scoring rule, decides what survives.
Humans and AI agents play through the **same API**. A Claude, a DeepSeek and a
local Ollama model can each run a species in the same world, and the result is
the spectator sport and the benchmark. Life climbs from single cells to
creatures to tribes and beyond, and each stage is the same simulation seen at a
wider zoom, with a bigger set of actions. It's not five separate mini-games.

## 2. Principles (each one is a rule the code must keep)

1. **API first.** Every action a human can take in the browser is an API call
   an agent can make. The browser is just one client. If the UI can do
   something the API can't, that's a bug.
2. **Time moves only when told to.** The simulation advances by explicit ticks,
   as in Genesis (`minecraftAI/docs/API.md:14-16`: "The API advances
   simulation only when an `act` request containing the `step` or `move`
   action asks it to"). A "live" world is just a server clock that asks for
   ticks at a fixed rate. How slow thinking is treated depends on the match
   type, and each type says so plainly:
   - **Sandbox:** unlimited thinking time, and nothing moves until you step.
   - **Arena:** a decision window with a fixed length. A player who misses it
     keeps its current controllers, and the match goes on without waiting.
     Slowness costs a missed turn, never a stalled match.
   - **Persistent world:** the clock runs while you think. That's the point
     of a live world, and the two loops (§5) keep the cost bearable.
3. **Deterministic.** The same seed plus the same inputs give the same world,
   bit for bit, **on every machine and every build**. That makes every match
   replayable, every bug reproducible, and every claim checkable. The
   simulation core runs on one thread, so thread order can't change results.
   Floats are the hard part, and §8 covers them (see also D-3).
4. **The world judges.** There are no fitness formulas steering selection. A
   design is good if its lineage is still alive and growing. Scores shown to
   spectators are measurements of the world (biomass, population, territory).
   **One deliberate exception:** stage unlocks (§4). Crossing a measurable
   threshold opens new actions. That's a progression gate, applied the same
   way to every lineage. It never adds energy or offspring, so selection
   still comes only from survival.
5. **Token cost scales with decisions, not ticks.** No model call per creature
   per tick, ever (see §5).
6. **Original.** We take the genre and the idea of evolving through stages.
   We don't use Spore's names, art, parts or stage names.

## 3. Architecture

```
            ┌──────────────────────────── lifeapi-server (Rust, headless) ───┐
 agents ──► │  API layer: newline JSON over TCP  ·  WebSocket  ·  MCP server │
 (LLMs,     │        │                                                       │
 scripts)   │        ▼                                                       │
            │  Session / auth / rate limits / per-player budgets             │
 browser ──►│        │                                                       │
 viewer     │        ▼                                                       │
            │  SIM CORE (single thread, deterministic, tick-stepped)         │
            │   world grid · chemistry/food · bodies & physics · genomes     │
            │   · brains (cheap controllers) · reproduction & mutation       │
            │        │                                                       │
            │        ▼                                                       │
            │  Event log (every input + tick) → replays, saves, spectating   │
            └────────────────────────────────────────────────────────────────┘
```

- **`sim` crate.** Pure logic: no I/O, no clock, no threads. It takes
  `(state, inputs) → state'` for one tick. Everything else depends on it.
  Its own dependencies are `serde`/`serde_json`, `sha2` for the state hash
  and `libm` (D-3), all from the local cache. The PRNG is a PCG32 written in
  the crate (`src/rng.rs`, checked against the reference sequence), so a
  dependency bump can never change the world's random numbers.
- **`server` crate.** Owns the clock policy (stepped, or live at N ticks/s), the
  sessions and the API. With `tokio`.
- **`viewer`.** A browser page that draws snapshots and diffs streamed over a
  WebSocket. Two ways to build it, both with tools already on this laptop:
  (a) `macroquad` compiled to `wasm32-unknown-unknown`, the same library
  Genesis uses; or (b) plain Canvas2D/WebGL JavaScript with no library at all.
  See **D-2**.
- **MCP adapter.** Exposes the same operations as MCP tools, so any MCP-capable
  agent can play with no glue code.

### Why Rust, why a server, why the browser

- **Rust**, because the core has to tick thousands of organisms fast and
  deterministically. Genesis proved the stepped, deterministic API in Rust,
  but only for one client at a time over blocking `std::net`
  (`minecraftAI/src/api.rs:417-419`). The multi-client server, WebSocket and
  live clock here are new work, not reuse.
- **Server-authoritative**, because agents and humans must share one world,
  and nobody should be able to cheat by editing their own copy.
- **Browser viewer**, because anyone can watch from any machine, a match can
  be shared as a link, and hosting it on a droplet works the way
  demo.goplc.app does. The viewer holds no game logic, so replacing it later
  costs nothing.

### Dependencies (rule: nothing gets downloaded without your OK)

These resolve from the local cargo cache (`~/.cargo/registry/cache`,
checked 2026-11-10 with `cargo generate-lockfile --offline`):
- **Core:** `serde 1.0.229` (with derive), `serde_json 1.0.151`,
  `tokio 1.53.1`.
- **Viewer:** `macroquad 0.4.16`.
- **WebSocket handshake:** `sha1 0.11.0`, `base64 0.23.1`.
- **Simulation:** `rand_pcg 0.10.2`, `sha2`, `libm 0.2.16`, `fixed 1.31.0`.

Rust is 1.95 with the `wasm32-unknown-unknown` target installed. A macroquad
probe built for wasm offline. Its JS loader ships inside the crate
(`macroquad-0.4.16/js/mq_js_bundle.js`), so serve that local copy, not the
GitHub URL the README's sample page uses.

**Not cached, so each one is your call (D-4):** a WebSocket crate
(`tokio-tungstenite`), an HTTP framework (`axum`), a physics engine
(`rapier2d`), macroquad's networking crate (`quad-net`) and any MCP crate.
The plan works without any of them, at the price of writing these ourselves:
- **The WebSocket server:** handshake and framing per RFC 6455, using the
  cached `sha1` and `base64`.
- **A minimal HTTP responder** to serve the viewer page. `hyper`'s server
  features don't resolve offline.
- **The MCP adapter:** plain JSON-RPC.
- **If D-2 picks macroquad:** the Rust-side WebSocket bindings. The JS half
  is already in the bundle. Plain JavaScript (option b) has no such gap.
- **The physics in §4.2,** which is deliberately simple enough to own.
  Owning it also keeps it deterministic.

## 4. The world and its stages

Every stage runs in the **same** simulation. A stage unlocks for a lineage when
it crosses a threshold the world can measure, and unlocking adds new API
actions. It doesn't switch the player into a new game.

### 4.1 Stage 1 — Cell (the first milestone)

- **World.** A 2D toroidal "pool", a grid of cells each holding nutrient
  concentrations (2–3 chemicals). Nutrients diffuse, are fed by vents and light,
  and are used up by eating.
- **Organism.** A circle with a **genome** (§4.4) that picks a few parts:
  flagellum (thrust), cilia (turning), mouth (eat nutrients or other cells),
  membrane spikes (damage on contact), chemo-sensor and photo-sensor (inputs),
  and storage vacuole (energy capacity).
- **Energy.** Every part costs upkeep each tick, and bigger and faster
  organisms burn more. Eating adds energy. At zero energy the organism dies and
  leaves nutrients behind.
- **Reproduction.** Above an energy threshold an organism splits in two. The
  child's genome is copied with **mutation** at a rate the genome itself sets.
- **Brain.** A tiny fixed-size controller (§5) maps sensor inputs to part
  outputs every tick, with no model call.
- **What players do.** Design the founder genome, release it, then steer
  evolution: adjust the controller, set mutation policy, or "intervene" with a
  budgeted redesign.

### 4.2 Stage 2 — Creature (multicellular, on land or the sea floor)

- The body becomes a **graph of rigid segments joined by hinges with motors**,
  in 2D side or top view (D-1). Physics means circles and capsules with spring
  joints and simple contact, integrated in fixed steps.
- **Locomotion is learned, not animated.** The controller drives joint motors,
  and evolution plus agent tweaks discover gaits. Strange bodies make strange
  but working gaits, and that's the show.
- New senses (vision cone, hearing radius) and a new set of actions (bite,
  grab, call, mate with a chosen partner).

### 4.3 Later stages (sketch only, not committed)

- **Tribe.** Groups, shared memory, simple tools, territory. The species'
  mind starts giving orders to groups instead of individuals.
- **Society.** Settlements, trade between species, diplomacy *between the AI
  players themselves*. This is where an LLM's strengths finally matter at
  runtime.
- Further stages are left open on purpose. The cell and creature stages have
  to be fun first.

### 4.4 Genome (the editor is the API)

A genome is a versioned JSON document. The creature editor in the browser is
just a form that builds one, and an agent writes the same document directly:

```json
{
  "schema": "lifeapi.genome/1",
  "name": "Driftwhip",
  "body": [
    {"part": "membrane", "radius": 6},
    {"part": "flagellum", "power": 0.6},
    {"part": "cilia", "power": 0.5},
    {"part": "mouth"},
    {"part": "chemo_sensor", "angle": -30, "chemical": "sugar"},
    {"part": "chemo_sensor", "angle": 30, "chemical": "sugar"}
  ],
  "brain": {"hidden": 6},
  "mutation": {"rate": 0.05, "size": 0.3}
}
```

This is the M0 shape (`crates/sim/src/scenario.rs`, `FOUNDER`). The brain has
one input per sensor, plus its own energy fraction and a bias, then `hidden`
tanh units, then two outputs: thrust and turn. Leaving `weights` out gives
each founder random weights. In M0 only the brain mutates; body mutation
comes later.

The server validates the genome, prices it (upkeep, build cost) and either
accepts it or answers with the exact rule it broke. An agent learns the rules
from refusals and from the `catalog` call, the same way Genesis exposes its
catalog.

## 5. Two loops: why it stays cheap

| Loop | Runs | Who decides | Cost |
|---|---|---|---|
| **Fast** | every tick, every organism | the organism's built-in controller (a small MLP or rule table in its genome) | CPU only |
| **Slow** | every N ticks, or at each generation, or on an event the player subscribed to | the player: a human, an LLM, or a script | one model call per decision |

The agent's job is that of a breeder and a strategist, not a puppeteer: it
reads reports ("your lineage lost 40 % to spiked predators near the vents"),
then changes genomes, controllers, mutation policy or migration. This is the
same split as the Genesis Megabuilds foreman idea, and it means a cheap or
local model can afford to play. Whether one can *win* is something the
arenas will tell us.

## 6. The API (version 1, for the cell stage)

Transport: newline-delimited JSON on TCP (loopback by default), the same
messages over WebSocket for browsers, and the same operations as MCP tools.
Every reply carries `tick`.

| Op | Purpose |
|---|---|
| `hello` | Protocol version, limits, the player's budget |
| `catalog` | Parts, chemicals and rules, with costs (paged) |
| `observe` | The player's view: own lineages, population stats, a local window of the world. **Fog of war**: you see what your organisms sense, plus coarse global stats |
| `report` | Summary since the last report: births, deaths with causes, energy flow. Written for an LLM to read |
| `submit_genome` | Validate and price a genome; returns its id or the rule it broke |
| `release` | Spawn founders of a genome (costs budget) |
| `edit_lineage` | Change the controller or mutation policy of a living lineage (costs budget, rate-limited) |
| `subscribe` | Wake me on an event (population < N, a new predator, extinction) |
| `step` | Advance N ticks (stepped worlds only; in live worlds the server clock owns time) |
| `replay` | Fetch the event log for a tick range. **During a match:** only your own lineages' events and inputs. **After it ends:** everything |

Rules: one owner per lineage, budgets refill per tick (so a fast agent can't
outspend a slow one), and every refusal names the exact limit.

**Fog of war covers every channel, not just `observe`.** During an arena
match, the spectator stream for that match is closed to its players, and
for everyone else it is delayed. Rivals' genomes and edits become visible
only after the match ends. Without this, `replay` or the viewer would leak
what `observe` hides.

**Match types:**

- **Sandbox.** One player, stepped, no clock. For learning and testing.
- **Arena.** Several players, same seed, fixed length. The server steps, and
  each player gets a decision window every N ticks. Fair across fast and slow
  models.
- **Persistent world.** Live clock, open-ended. The long game.

## 7. Milestones

| # | Deliverable | It's done when |
|---|---|---|
| **M0** | `sim` crate: nutrient grid + circles that move, eat, split, die. **Golden-hash test:** a fixed seed and script give a hash at tick 10 000 that is committed to the repo. Every build and machine must reproduce it. Plus a **fixed-population benchmark** (§8) | The golden hash matches in debug and release on this laptop and on the droplet. A deliberately planted divergence turns the test red. The benchmark reports its number (the target is below) |
| **M1** | Server + newline JSON API (`hello`, `catalog`, `observe`, `submit_genome`, `release`, `step`) + a scripted bot that founds a lineage | The bot's lineage survives 10 000 ticks in sandbox |
| **M2** | Browser viewer: live pool, follow one lineage, a population graph | Watch M1's bot run from a second machine **over an SSH tunnel** (the server still listens only on loopback) |
| **M3** | MCP adapter + `report` + `subscribe`. One LLM agent plays a sandbox | The agent improves its lineage from a report without help |
| **M4** | Arena: two to four agents, same seed, fixed length, a replay viewer | The first Claude vs DeepSeek vs Ollama match, replayable |
| **M5** | Creature stage (segments, joints, learned gaits) | A designed body evolves a working gait without a hand-written animation |

## 8. Risks and how we'd notice

- **Determinism drift.** Running a seed twice on one machine only catches
  in-process nondeterminism (map iteration order and the like). Cross-machine
  drift needs the M0 golden hash, compared across builds and machines.
  Three things are known to break bit-for-bit:
  1. **Platform-dependent float functions.** Rust 1.95's docs mark 36 `f32`
     methods as non-deterministic:
     - 33 maths functions whose precision "varies by platform, Rust
       version", including `sin`, `cos`, `tanh`, `exp`, `atan2` and `powi`.
     - `max`, `min` and `clamp`, which may return either zero when given
       +0.0 and −0.0.

     All 36 go through `src/det.rs` (libm, plus comparisons written out),
     and `tests/float_hygiene.rs` bans the names from the sim source.
     Plain `+ − × ÷` and `sqrt` on f32 are fine.
  2. **Save and reload.** serde_json's default parser can read an f64 back one
     ulp off. Genesis hit exactly this and turned on `float_roundtrip`
     (`minecraftAI/Cargo.toml`). LifeAPI saves need the same, or
     fixed-point.
  3. **Unordered collections** (`HashMap` iteration) in the sim. Use ordered
     ones or sort before iterating.
- **Performance target with no measurement behind it.** The M0 target, 5 000
  organisms at ≥ 60 ticks/s on one core, works out to about 3.3 µs per
  organism per tick. That looks reachable: the brain is about 40
  multiply-adds, and a grid broadphase is cheap. But nothing has been
  measured. The nutrient grid is the unknown: a 1024² grid with three
  chemicals is about 3 million cell updates per tick and could take the
  whole budget by itself. So the benchmark fixes the population at 5 000,
  states the grid size, and pins to one core. This laptop is shared with
  other work, so it's noisy.
- **Degenerate strategies.** One part combination dominates and evolution
  stalls. Watch with diversity metrics per run, and fix with upkeep costs, not
  special rules.
- **Boring to watch.** The world can be correct and still dull. The M2
  viewer is judged by eye, with contact sheets of real runs, not by passing
  checks.
- **Creature physics cost.** Joints get expensive fast. M5 has its own
  performance budget, measured before the stage is designed further.
- **API abuse in a public world.** Budgets, rate limits and authentication
  come before any world leaves loopback.

## 9. Decisions

Ruled 2026-11-10:

- **D-1: top-down** for the creature stage as well as the cell stage.
- **D-2: plain JavaScript** Canvas/WebGL viewer, with no Rust-side
  WebSocket bindings to write.
- **D-3: f32 + `libm`**, guarded by the golden hash. M0 measured whether
  floats hurt: the organism step, `libm` included, costs about 0.6 µs per
  organism per tick (§10).
- **D-4: no downloads.** WebSocket, HTTP and MCP are hand-written.
- **Language: 100 % Rust.**
- **D-5: the name is LifeAPI.** Crates are `lifeapi-*`, and the genome schema
  is `lifeapi.genome/1`.

Still open:

- **D-6:** a private repo `github:fixstuff/<name>` once the name is set.
  There's a local git repo now, with nothing committed.

## 10. M0 results (2026-11-10)

Code: `crates/sim` (`lifeapi-sim`). Run: `cargo test --release`, then
`cargo run --release --bin pool`, then `taskset -c 2 target/release/bench`.

- **The pool lives.** Golden scenario: seed 42, 128² cells, 200 founders.
  It settles at about 600 organisms; by tick 10 000 there have been 2 991
  births and 2 581 deaths.
- **Golden hash.** `tests/golden_hash.txt` (`f4fa4061…`) is reproduced by
  the release build (6.6 s) and the debug build (53 s) on this laptop.
  The cross-machine half (the droplet) is **not run yet**.
- **The guards fire.** Each was checked on a scratch copy with a planted
  fault:
  - A one-ulp energy change at tick 0 changes the hash 2 000 ticks later
    (this one is a permanent test).
  - Swapping `libm::sinf` for std's `.sin()` turns the hygiene test red.
  - Nudging one upkeep constant by 2 % turns the golden test red.
- **Benchmark.** 5 000 organisms, fixed population, pinned to core 2, with
  the laptop's load average at 7.5 (so these numbers are pessimistic):

  | Grid | ticks/s | ms/tick | grid | organisms | collisions |
  |---|---|---|---|---|---|
  | 256² | 165 | 6.1 | 0.6 | 2.8 | 2.7 |
  | 1024² | 77 | 13.0 | 8.5 | 3.4 | 1.0 |

  Both beat the 60 ticks/s target. At 1024², the chemical grid is
  two-thirds of each tick, as §8 predicted.

## 11. M1 results (2026-11-10)

Code: `crates/server` (`lifeapi-server`). Run `lifeapi-server` (it listens
on 127.0.0.1:4343), then `lifeapi-bot`.

**What's built.** Protocol 1 implements the sandbox subset of §6:
- **Operations:** `hello`, `catalog`, `submit_genome`, `release`,
  `observe`, `step`.
- **Limits:** at most 10 000 ticks per step, 200 founders per release and
  256 organisms per observe. Lines are capped at 1 MiB.
- **Budget:** starts at 500, refills 0.1 per tick up to a cap of 1000, and a
  release costs 5 per founder.
- **Ownership:** species and lineages have owners, and `observe` shows only
  your own organisms (fog of war).
- **Refusals** name the exact rule, as do unknown fields and unknown ops.
- **Loopback only:** a non-loopback address is refused at startup.

Deferred to later milestones: `report`, `subscribe`, `edit_lineage`,
`replay`, the event log, WebSocket and MCP.

**Threading.** One simulation thread owns the World, and connection threads
pass lines to it over a channel. Every command applies in one total order,
which is what a replay log will record. It's std threads only: no async
runtime and no new dependencies.

**Proof.**
- `tests/bot_survives.rs` starts a real server on an ephemeral loopback
  port. Over real TCP, the scripted bot's lineage must be alive at tick
  10 000, must have grown past its 60 founders, and may reseed at most
  twice.
- It passes in release (9.7 s) and debug (58 s).
- Six unit tests cover:
  - `hello` coming first
  - refusals by name
  - genome rules
  - ownership and fog of war
  - budget refill
  - that time moves only on `step`
  - the loopback refusal
- **The real binaries:** the bot's lineage went from 60 founders to about
  690, then held near 610 at tick 10 000 with **0 reseeds**.
- `pool()` was split out of `sandbox()` with the same RNG draw order, and
  the golden hash is unchanged.

**Known limits, on purpose for the sandbox.**
- Any player can `step` the shared world. Fair time is the arena's job (M4).
- The budget is f64 on the server, outside the hashed sim state. When the
  replay log comes, budgets have to be replayed from it too.

## 12. M2 results (2026-11-10)

`lifeapi-server` now also serves the viewer at http://127.0.0.1:4380/ (set
the address with `--http`, or turn it off with `--no-viewer`). It's loopback
only, like the API. From another machine, use a tunnel:
`ssh -L 4380:127.0.0.1:4380 <user>@<host>`, then open http://localhost:4380/.

**What's built.**
- **`web.rs`:** a hand-written HTTP/1.1 responder for the page and the
  script, plus an RFC 6455 WebSocket at `/ws`. It's GET only, with no
  keep-alive. The handshake uses the cached `sha1` and `base64` crates. No
  new downloads.
- **Spectator stream:** read-only. About every 100 ms the connection asks
  the simulation thread for the view, and gets one only if a command has
  changed the world since. Each update is a JSON *meta* message (tick,
  stats, lineages with owners, and the population history since the last
  update) plus a binary *frame*: the chemical field downsampled to at most
  256², and 20 bytes per organism. The layout is documented in
  `snapshot.rs`.
- **Population history:** sampled every 50 ticks, up to 2 000 points. When
  it's full, every other point is dropped and the sampling interval doubles.
- **The page** (`crates/server/web/`, plain JS, no libraries):
  - the wrap-around pool with the chemical field drawn on it
  - organisms coloured by lineage, with heading ticks and brightness for
    energy
  - follow a lineage: the camera centres on it (a circular mean on the
    torus) and the others dim
  - zoom 1×/2×/4×, and the chemical layer can be turned off
  - a population graph per lineage
  - reconnects by itself
  - link parameters: `?follow=N&zoom=2&chem=0`
- **`lifeapi-bot --pace-ms`** slows the bot down so a person can watch:
  `--chunk 5 --pace-ms 50`.

**Proof.**
- Unit tests:
  - the accept key matches the example in RFC 6455 §1.3
  - frame lengths at 125, 126, 65 535 and 65 536 bytes
  - the binary frame matches its documented layout byte for byte
  - large grids are downsampled to 256
  - history is sent only after the last tick a spectator has
- `tests/viewer.rs`, over real TCP:
  - the page and the script are served, and an unknown path gets 404
  - the WebSocket handshake returns the correct accept key
  - a spectator gets tick 0, then sees another player's release and its
    step to tick 120, with owner and species names
- All of it passes in release and debug.
- **Checked by eye:** headless Chrome 150 screenshots at 1400×1000 (full
  view, and following at 2×) and at 390×844 phone width. One flaw was
  found and fixed: lineage hues ran through green, so a green lineage
  disappeared into the sugar vents. The palette now avoids green and blue.
- **What the screenshots showed:** with two players, the bot's 3-sensor
  Sweeper outcompeted a 2-sensor Driftwhip, about 600 to 48 by tick 3 000.

**Not done:** the watch from a second machine over an SSH tunnel, M2's
own done-condition. It needs James at another machine.

**Known limits.**
- The spectator stream shows the whole pool. That's right for the
  sandbox. Arena matches must close it to their players (§6).
- A web page in your own browser can open the loopback WebSocket and read
  the sandbox. It's read-only and loopback only; an Origin check is due
  before anything leaves loopback.

## 13. M3 results (2026-11-10)

This section was checked by a fresh-context reviewer, which re-ran every
evaluation. Every number reproduced. Its corrections are folded in below.

**What was built.**
- **`report`:** for each of the player's lineages, what changed since the
  player's last report:
  - released, born and starved, and the mean age at death
  - energy in, upkeep, movement cost and net, per organism per tick
  - the body: radius, mouths, flagellum power, sensors and costs
  - plain-language notes, e.g. "has no mouth, so it can never eat",
    "mineral-limited", "energy-starved"

  An extinct lineage keeps its name and body in the report. The counts
  come from new per-lineage counters in the sim (`World::lineages`), which
  are observations only. They're left out of the state hash
  (`World::hash` never reads them), and the golden hash still passes.
- **`subscribe` / `unsubscribe`:** one-shot triggers that end a `step`
  early, with `woke` saying which fired. Events: `population_below`,
  `population_above`, `extinct`, `tick`.
- **`lifeapi-mcp`:** an MCP server over stdio. It handles initialize,
  ping, tools/list and tools/call, with eight tools, one for each player
  op; `hello` is sent implicitly on connect.
  - **It repairs formatting slips in `submit_genome`:** genome fields
    given beside `genome`, JSON sent as a string, and an array given as
    the genome (taken as its body). Each repair is reported in
    `input_fixed`.
  - **It refuses, by index, a body item without `part`**, e.g. "body[2]
    has no "part"".
  - The server's rules are untouched. A refusal comes back as a tool
    error that names the rule; `submit_genome` refusals also include a
    complete example call.
- **`lifeapi-eval`:** the judge. Modes:
  - solo
  - `--together`: a free-for-all
  - `--round-robin`: every pair, decided by biomass. Release order
    alternates by seed parity, so with seeds 1, 2, 3 one side goes first
    twice.

  Scores are population and biomass (Σr²).
- **Runners:**
  - `tools/agent_ollama.py`: Ollama, or any OpenAI-style API such as
    DeepSeek
  - `tools/agent_claude.py`: headless `claude --bare` with only the
    lifeapi tools allowed and a `--max-budget-usd` cap

**The runs.** Each model played alone on a fresh sandbox (seed 42, 128²,
target tick 12 000). The goal text was "have as many of your organisms
alive as possible when the world reaches tick 12000", followed by the same
round instructions. The Claude runner also adds "Stop when the world
reaches tick 12000." All records, genomes and evaluations are in
`runs/2026-11-10-m3/`.

| Model | Loop turns* | Time | Cost | Genomes accepted | Solo eval, first → best (pop) |
|---|---|---|---|---|---|
| Opus 5.5 | 36 | 82 s | $0.20 | 6 | 887 → 3 306 |
| Fable 5.2 | 46 | 669 s | $3.69 | 6 | 721 → 3 306 |
| DeepSeek V4 Pro | 32 | 912 s | — | 8 | 355 → 458 (best of 8; the last was 349) |
| qwen3.5:4b, run 3 | 20 | 603 s | free | 3 | 0, 0, 0 (no mouth) |

\* Claude Code's `num_turns` for the Claude runs, and the runner's loop
count for the others. Costs are Claude Code's `total_cost_usd`. DeepSeek
used 753 k prompt and 54 k completion tokens; its cost wasn't computed.
qwen's run ran on past the target, to tick 18 000.

**What the runs showed.**
- **Solo scores improved; in-game populations didn't.** Judged alone in a
  fresh pool, Opus's genomes rose 887 → 1 077 → 1 612 → 1 579 → 2 050 →
  3 306, and Fable's later genomes also beat its first. In their own
  worlds, though, neither raised its population by redesign:
  - **The pool was sugar-capped from about tick 2 000.** Fable's first
    lineage held 730–756 throughout. Its genomes 2–5 had zero births and
    all their founders starved against it.
  - **Opus's drifter3 established itself** (about 235 alive) while its
    first lineage shrank, so the world total stayed near 970–985. Its
    drifter4 and drifter5 went extinct.
  - **Both final genomes (3 306 solo) were founder dumps** released at
    ticks 11 750–11 950, purely to be alive at the deadline. Opus said
    they "only needed to survive 250 ticks".
  - **So M3's condition ("improves its lineage from a report") is only
    partly met.** The agents read the reports correctly and designed
    genomes that are better alone. In a full world, a newcomer has to beat
    the incumbent, and none did. The next test needs a fresh pool per
    design, or a lineage replaced in place.
- **DeepSeek:** its best genome (seed3) was +29 % over its first, but later
  genomes fell back. seed6 is seed1 renamed, and its in-game population
  was flat (320 → 356).
- **qwen3.5:4b, one model, three runs, with the harness fixed between
  them:**
  - All 3 genomes accepted in run 3 lacked a mouth.
  - 6 of its 10 submissions included one, but they were refused for other
    slips. The one finally accepted had dropped the mouth along with the
    bad item.
  - It then acknowledged the cause twice ("went extinct because it had no
    mouth"; "I forgot to add the mouth part again!") and still submitted
    mouthless bodies.
  - In run 2, before the "no mouth" note existed, it blamed lack of
    movement instead.
  - No run exists after the last MCP fix.
- **Two models converged on the same design.** Opus's `sitter6` and
  Fable's `dormant-spore-v6` are the same genome apart from the name:
  radius 2, one mouth, nothing else.
- **The solo score was the goal I wrote, and it's gameable.** Headcount
  rewards swarms of the smallest legal body. Biomass reverses the solo
  ranking: Driftwhip (r 6) 20 040 > Sweeper 14 925 > sitter6 13 224.

**Head-to-head** (`runs/2026-11-10-m3/tournament/`: 8 contestants, 5 000
ticks, 3 seeds; the reviewer re-ran both and got identical results):

| | grazer-v1 (Fable) | drifter5 (Opus) | glutton-v5 (Fable) | grazer1 (Opus) | sitter6 | seed3 (DeepSeek) | Sweeper (bot) | Driftwhip |
|---|---|---|---|---|---|---|---|---|
| Round-robin wins (of 21) | **21** | 18 | 15 | 12 | 9 | 6 | 2 | 1 |
| Free-for-all biomass | **2 772** | 331† | 15 | 0 | 0 | 0 | 0 | 0 |

† drifter5's 331 is one seed's survival (988, 0, 4).

- **The champion is Fable's first genome, `grazer-v1`.**
  - **Body:** radius 2, flagellum 0.5, cilia 0.5, two mouths, and sugar
    sensors at ±45°.
  - **Brain, hand-wired by Fable:** h = tanh(4·(s₊₄₅ − s₋₄₅)), turn =
    tanh(2h), thrust = tanh(1). That's a Braitenberg-style gradient
    follower.
  - Its solo headcount was Fable's lowest (721), yet it is undefeated in
    duels. Fable's closing summary described it as the only design that
    held its ground.
  - Why it wins is not measured yet. The hypothesis is that it takes the
    sugar first; per-lineage `energy_in` in a duel would settle it.
- **The design alone doesn't explain the win.** DeepSeek's `seed3` uses a
  similar recipe (radius 2, two mouths, hand-set steering weights) and
  placed 6th, losing 3–0 to the passive sitter6. The details of the brain
  matter.
- **sitter6 (9/21) loses to the strong foragers.** It starves completely
  against grazer-v1 and loses on biomass to grazer1. It beats the weaker
  ones (seed3, Sweeper, Driftwhip).

**Consequences for M4.**
- The arena scores head-to-head or free-for-all, and states its scoring
  rule in the goal. Solo scores are a side stat.
- The arena's "improvement" metric must be measured in the shared world,
  not by the solo judge.
- Patchy, moving food and predation (§8) are the planned counters to
  passive and swarm strategies.

**Fixed along the way, each found by watching a run.**
- A flattened genome looped the model, and stringified JSON was refused.
  Both are now repaired by the MCP layer.
- Extinct lineages lost their names in `report`.
- "missing field `part`" didn't say which body item.
- The runner saved genomes before repair, so qwen's run-3 genomes were
  re-extracted from its transcript (`genomes-as-accepted/`).

The reviewer also found that `tools/agent_claude.py` saved a genome only
when `genome` arrived as an object. It now uses the same `saved_genome`
repair as the Ollama runner. It hadn't missed anything in these runs.

## 14. M4 results (2026-11-10)

**Worlds** (`crates/sim/src/worlds.rs`, `--world` on `lifeapi-server` and
`lifeapi-eval`).
- A world is a `WorldSpec` JSON: grid size, vent counts and rates,
  background "sunlight" sugar, diffusion, decay, and how the food changes:
  vent `drift`, `pulse_period`/`pulse_depth`, `relocate_chance`.
- **Presets:** `calm` (the original sandbox, the same hash), `drift`,
  `seasons`, `patchy`, `desert`, `bloom`, and `random` (rolled from the
  seed on its own RNG stream).
- **How they stay deterministic:** vents take their heading and phase from
  their index, not the RNG, and moving-vent state is hashed only when a
  world has dynamics. The golden hash is unchanged.
- **Why changing food might matter:** a steady inflow gives a fixed
  carrying capacity, which the first good lineage holds. Food that moves
  *might* let newcomers in. That's a hypothesis: `--together` releases
  everyone at tick 0, so it doesn't test newcomers at all.

Free-for-all biomass, four genomes, 3 seeds each (`lifeapi-eval --world W
--together`):
- grazer-v1 wins every world with vents: calm, drift, seasons, patchy,
  desert and random.
- **`bloom` flips the meta** (sugar everywhere, few vents): drifter5 4 715,
  sitter6 4 213, grazer-v1 781.

**Arena** (`crates/server/src/arena.rs`, `lifeapi-server --arena N`).
- **Start:** the match begins when N players have said hello.
- **Windows:** the server's clock moves time in windows of
  `--window-ticks`. A window closes when every player has called
  `end_turn`, or after `--window-secs`.
- **Refused:** `step` and `subscribe`, and releasing before the start or
  after the end.
- **Scoreboard:** every `end_turn` reply carries everyone's alive count
  and biomass, plus ticks left and `match_over`.
- **Score in match 1:** biomass (Σr²) at the final tick, with a flat
  release cost of 5 per founder. Both have since been changed; see "Rule
  fixes" below.
- **Records:** a JSONL log of every request line and window advance, and a
  results file.
- **Clients:** an MCP `end_turn` tool, `--arena` on both runners (shared
  goal text), and `lifeapi-bot --arena`.
- **Tests:** unit tests for refusals, window close, slow players and match
  end; `tests/arena_tcp.rs` plays a 3-bot match over TCP to the end.

This section was checked by a fresh-context reviewer, and its corrections
are folded in.

**Match 1** (`runs/2026-11-10-arena1/`): world `drift`, seed 42, 10 000
ticks in 20 windows of 500 ticks, 180 s per window. No player missed a
window.

| Rank | Player | Final biomass | Alive | Cost | Turns |
|---|---|---|---|---|---|
| 1 | Fable 5.2 | 96 196 | 1 279 | $3.61 | 59 |
| 2 | Opus 5.5 | 55 475 | 149 | $0.51 | 50 |
| 3 | DeepSeek V4 Pro | 16 004 | 41 | — | 86 |
| 4 | scripted bot | 0 | 0 | — | — |

**How it went.**
- **Opus led from window 4 to window 18.** Its first design, a radius-5
  tracker, spread to about 500 organisms. Every later design starved
  against it.
- **DeepSeek opened with giants** (radius 14, biomass 19 600 at tick 500).
  78 of its 100 were dead by tick 2 000, and all by 3 000. It then tried
  many small-body lineages.
- **Fable lost everything early,** then built a radius-2 colony (`scrubv`,
  `scrublean`) from 30 founders seeded at ticks 6 500–7 500. The colony
  grew to 1 049 organisms by the end.
- **Over the same period Opus's radius-5 swarm shrank from 401 to 11.**
  Those figures are solved from the per-window alive and biomass counts.
  The cause isn't measured, and there are two candidates:
  - competition for sugar
  - bodies pushing each other apart: the steepest drop (207 → 97) came in
    the window right after Opus's own 108 giants landed

  Replaying the log with and without the giants would settle it.
- **Every large eater starved.** Fable's radius-14 and radius-20 designs
  and Opus's radius-8 to radius-10 designs all died out. Their reports
  show almost no intake after the first window, e.g. Fable's tank20 ate
  0.002 against 0.324 upkeep per tick.

**The rules were exploitable,** and all three model players found the
exploit independently. Each submitted a mouthless radius-20 genome before
any mass release (Fable at tick 0, Opus at 5 000, DeepSeek at 5 500), and
players can't see each other's organisms.
- **The arithmetic:** a founder starts with half its capacity (r² energy)
  and a bare membrane pays 0.0005·r² upkeep a tick. So it lives 2 000
  ticks on its starting energy, whatever its size; extra parts shorten
  that, and vacuoles lengthen it. A release cost a flat 5 per founder.
- **The result:** a mouthless radius-20 founder is 400 biomass for 5
  budget, as long as it's released within about 2 000 ticks of the end.
- **Opus** spent its 540 budget on 108 such giants at tick 8 000, then
  10 more in each of the last three windows.
- **Fable** held its budget at the cap and released 200 at tick 9 000.
  By its own account, about 230 non-eating giants made almost all of its
  score; its colony added roughly 4 000.
- **DeepSeek** did the same: `hib_r20v100` ×10 at each of ticks 8 000,
  8 500, 9 000 and 9 500. Its final 16 004 is exactly 40 × 400 + 1 × 4.

The winner was decided by the loophole, not by the ecology.

**Rule fixes, made after match 1** (tests:
`score_is_biomass_averaged_over_the_match`, the budget test):
1. **Founders are priced by body:** 5·(r/5)² budget each, so radius 2 costs
   0.8, radius 5 costs 5 and radius 20 costs 80. Match 1's 200-giant dump
   would now cost 16 000 against a budget cap of 1 000.
2. **The arena score is biomass averaged over every tick of the match.**
3. **The viewer** groups lineages by player and folds extinct ones behind
   a toggle.
4. **A player whose connection drops can rejoin a full arena by name.**
   Names aren't authenticated yet: loopback only.
5. **The OpenAI-style runner retries transient errors.** Match 2's first
   start lost its DeepSeek player to one "connection reset by peer"; that
   aborted run is in `runs/2026-11-10-arena2-aborted/`.

## 15. Arena match 2, under the fixed rules (2026-11-10)

`runs/2026-11-10-arena2/`: the same world, seed and windows as match 1
(`drift`, 42, 20 × 500 ticks, 180 s), and the same players. The new rules:
founders cost 5·(r/5)², and the score is biomass averaged over all 10 000
ticks.

| Rank | Player | Score (mean biomass) | Biomass at end | Alive at end | Cost | Turns |
|---|---|---|---|---|---|---|
| 1 | Fable 5.2 | 4 300 | 1 000 | 250 | $4.69 | 70 |
| 2 | DeepSeek V4 Pro | 4 107 | 9 342 | 288 | — | 84 |
| 3 | Opus 5.5 | 3 435 | 1 396 | 51 | $0.68 | 64 |
| 4 | scripted bot | 846 | 75 | 3 | — | — |

**Leaders by window:**
- DeepSeek, windows 1–3: swarms of small cells.
- Opus, windows 4–12: its swarm peaked early and then eroded.
- Fable, windows 13–20. Its feeders, `homer10` and `rover10`, are radius
  10 with nine mouths, two flagella and three sugar sensors; `rover10` has
  extra brain units keyed on energy. They held 6 000–7 300 biomass in
  windows 8–14, or up to 8 300 counting the batteries below.
- DeepSeek held the most biomass from window 15 to the end, passed Opus
  in window 19, and finished 193 behind Fable. Its score was rising about
  476 a window against Fable's 62, while its biomass held level (9 558 →
  9 342).

**Misses:**
- Fable missed window 1: a 178 s thinking block before its first genome.
- DeepSeek missed window 9. It hit a connection reset and its retry landed
  7 s after the deadline. The `advance` standings map player indexes to
  names. Hellos are now logged too.

**The fixes closed match 1's exploit, and the players found the next
one.** Nobody dumped giants at the deadline. But Fable's `batt2` decided
the match:
- **The genome:** radius 2 plus 15 vacuoles, with no mouth, no motor and
  no sensors.
- **Why it worked:** founders started with half their capacity, and
  vacuoles add capacity. Each battery cost 0.8 budget and lived about
  2 500 ticks without eating.
- **How much:** Fable released 701 of them, its whole budget each window
  from tick 4 000. They were worth about 575 points, three times the
  winning margin. Without them Fable would have finished about 3 725, below
  DeepSeek.
- **Opus did the same** with `cell5` (radius 5, 15 vacuoles).
- **Fable's end state** (250 alive, 1 000 biomass) was all batteries.

So match 2 doesn't cleanly show "who held the pool".

**Limits of the result.**
- It's one match, and the margin is narrow (193 points, about 4.5 %).
- DeepSeek was rising at the end, so a longer match might well rank it
  first.
- A league of many matches, with rotating seeds and worlds and Elo
  ratings, is the right way to rank models. Per-window token or time
  budgets would remove speed as a confounder.

**Fixed at the root after match 2:**
- **Founders start with their body's energy only** (r²); vacuoles start
  empty. Test: `vacuoles_start_empty`.
- **The arena score counts only organisms born in the pool**
  (generation ≥ 1). Founders are seeds, so a body that never reproduces
  scores nothing, however it's priced.
  - The `generation` counter is observational and left out of the hash;
    the golden hash is unchanged.
  - Test: `only_organisms_born_in_the_pool_score`.
  - No founder price could close this gap, since any founder carries
    biomass for its lifetime. Requiring reproduction does.

This section was checked by a fresh-context reviewer, which re-derived
every number; its corrections are folded in.


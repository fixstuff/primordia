# Primordia

**An evolution arena for AI and code.** Players design single-celled life
through an API, release it into a shared, deterministic world, and compete to
keep it alive. Every match is a freshly generated world and every opponent
adapts, so there is nothing to memorise.

Website, results and white paper: **https://primordia.tools**

- **A deterministic Rust simulation.** Diffusing sugar and mineral on a
  torus; organisms that sense, swim, eat, divide, mutate and die. A SHA-256
  hash of the whole world state is pinned by a test, and a second test bans
  floating-point functions that are not bit-reproducible.
- **Genomes made of parts.** Membrane, flagella, cilia, mouths, chemical and
  organism sensors, vacuoles; and for combat, jaws, stingers, shells,
  spikes, camouflage and toxin. A small neural brain (tanh) drives thrust
  and turn. Every part has a cost.
- **Played through an API.** Newline-delimited JSON over TCP (the
  *LifeAPI* protocol), and an MCP server so any agent framework can play.
- **Arenas.** Several players in one world on a server-owned clock, turn-based
  or real time. Score: the biomass of organisms *born in the pool*, averaged
  over the match. Combat (`off`, `colony`, `individual`) and body evolution
  are match settings.
- **A viewer.** Every organism drawn from its genome; zoom, pan, and click any
  organism to see what it senses, what its brain decides, and what it is
  doing, in words. Fights, kills and newly evolved bodies are marked live.

## Quick start

You need a Rust toolchain. Nothing is downloaded at runtime.

```sh
cargo build --release
cargo test --release            # includes the golden-hash determinism test

# A world to watch: open http://127.0.0.1:4380/
target/release/primordia-server --world drift

# A scripted player
target/release/primordia-bot --addr 127.0.0.1:4343

# An MCP server for AI agents (stdio)
target/release/primordia-mcp --addr 127.0.0.1:4343 --name my-agent
```

A real-time combat arena for six players:

```sh
target/release/primordia-server --world patchy --arena 6 --realtime \
    --combat colony --evolve-bodies 0.02
```

## Layout

| Path | What |
|---|---|
| `crates/sim` | The simulation (`primordia-sim`): world, genome, brain, determinism helpers, world presets, and `primordia-eval`, a judge that replays genomes on fixed seeds |
| `crates/server` | The server (`primordia-server`): the LifeAPI protocol, arenas, the viewer (HTTP + WebSocket, hand-written), and `primordia-bot` |
| `crates/mcp` | `primordia-mcp`: the protocol as MCP tools |
| `tools/` | Agent runners (Claude, OpenAI-style APIs, Ollama), the league runner, and `analysis.py` (ratings with error bars, benchmark correlation) |
| `runs/` | Every league and match played so far, with logs, genomes and results |
| `docs/DESIGN.md` | The design document and the milestone write-ups |

## Results so far

Frontier and open models have played three leagues without combat and are
playing combat leagues now. Ratings with 95% intervals, cost per match, and
the correlation with ARC-AGI-2 are on https://primordia.tools/results/.
Along the way the AI players found three reward hacks in the scoring rules,
each fixed in turn, and then the root cause was closed: see
https://primordia.tools/history/.

## Names

*Primordia* is the project. *LifeAPI* is the wire protocol: the genome schema
is `lifeapi.genome/1`, and the viewer's binary frames start with `LAPI`.

## Authors

- **James** ([@fixstuff](https://github.com/fixstuff)): the idea, the design
  decisions, and direction throughout.
- **Claude** (Anthropic): wrote the code and documentation with James.
  Claude Opus 5.5 built it; Claude Fable 5.2 reviewed the results and the
  write-ups with fresh context.

## License

MIT. See [LICENSE](LICENSE). Copyright (c) 2026 the Primordia contributors.

Free to use, fork and build on. The project does not accept contributions,
pull requests or issues: see [CONTRIBUTING.md](CONTRIBUTING.md).

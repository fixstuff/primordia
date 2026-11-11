#!/usr/bin/env python3
"""Run a LifeAPI league: many arena matches, ranked by Elo.

    python3 tools/league.py --out runs/league-1 --worlds drift,patchy,bloom --seeds 11,12 \
        --parallel 3 --fable-usd 6 --opus-usd 2

Every match is the same players (Fable, Opus, DeepSeek, the scripted bot,
and each --openai model: any API player in the OPENAI table, Gemini included) on a fresh arena server with its own
ports. A match whose results.json already exists is not run again, so a
league can be continued (League 2 added GPT-5.5 from match 4). Matches run --parallel at a
time. When a match ends, its results.json is read and the league table is
rebuilt:

- Elo: each match counts as every pair of players playing one game; the
  higher score wins, equal scores draw. K = 32 / (players - 1), so one match
  moves a rating about as much as one two-player game. Matches are applied
  in schedule order, so ratings depend on order; the table also shows
  order-free stats: wins, mean rank, and mean share of the match's top score.

Standard library only. Key files are read by the agent runners, never here.
"""
import argparse, itertools, json, pathlib, socket, subprocess, sys, time


def port_free(port):
    """True if nothing listens on 127.0.0.1:port or on any address (another
    service, e.g. a ControlForge instance, may hold it on 0.0.0.0)."""
    for host in ("127.0.0.1", "0.0.0.0"):
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            s.bind((host, port))
        except OSError:
            return False
        finally:
            s.close()
    return True


def free_pair(start, taken):
    """The first port p >= start with p and p+1 free and not already taken
    by this batch (League 4 lost two matches to ports 4841/4850 held by
    another program)."""
    p = start
    while not (p not in taken and p + 1 not in taken and port_free(p) and port_free(p + 1)):
        p += 2
    return p

ROOT = pathlib.Path(__file__).resolve().parent.parent
PLAYERS = ["claude-fable-5-2", "claude-opus-5-5", "deepseek-v4-pro", "scripted-bot"]
# Runner subfolder for each player whose summary.json carries cost_usd.
COSTED = {"fable": "claude-fable-5-2", "opus": "claude-opus-5-5"}
# OpenAI list prices from third-party sources, NOT checked against OpenAI's own
# page: input, cached input, output USD per million tokens; and whether the
# model needs the Responses API for function tools with reasoning on.
# Also: the OpenAI-style endpoint and key file for each model.
_OA = ("https://api.openai.com/v1", "openai.env", "OPENAI_API_KEY")
_GEM = ("https://generativelanguage.googleapis.com/v1beta/openai", "gemini.env", "GEMINI_API_KEY")
OPENAI = {"gpt-5.5": ("5,0.5,30", False) + _OA,
          "gpt-6-astra": ("10,1,50", True) + _OA,
          # $2/$10 introductory to 2026-11-30, then $4/$20; priced at the later rate.
          "gemini-4-argon": ("4,0.1,20", False) + _GEM}


def launch(match_dir, port, http, world, seed, a):
    """Start a server and the four players for one match. Returns the processes."""
    d = match_dir
    for sub in ("fable", "opus", "deepseek", "bot") + tuple(a.openai):
        (d / sub).mkdir(parents=True, exist_ok=True)
    addr = f"127.0.0.1:{port}"
    rel = lambda p: str(p)
    procs = {}
    procs["server"] = subprocess.Popen(
        [rel(ROOT / "target/release/lifeapi-server"), "--addr", addr, "--http", f"127.0.0.1:{http}",
         "--world", world, "--seed", str(seed), "--combat", a.combat, "--evolve-bodies", str(a.evolve_bodies), "--arena", str(len(PLAYERS)), "--window-ticks", str(a.window_ticks),
         "--window-secs", str(a.window_secs), "--first-window-secs", str(a.first_window_secs), *(["--realtime"] if a.realtime else []), "--match-ticks", str(a.match_ticks),
         "--log", rel(d / "match-log.jsonl"), "--results", rel(d / "results.json")],
        stdout=open(d / "server.log", "w"), stderr=subprocess.STDOUT, cwd=ROOT)
    # Players start only once the server answers: a server that failed to
    # bind must not leave agents spending money against a dead port.
    for _ in range(50):
        if procs["server"].poll() is not None:
            break
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            break
        except OSError:
            time.sleep(0.1)
    if procs["server"].poll() is not None:
        print(f"SERVER FAILED for {d.name}: see {d / 'server.log'}; players not started", flush=True)
        return procs
    py = [sys.executable, "-I"]
    procs["fable"] = subprocess.Popen(
        py + ["tools/agent_claude.py", "--arena", "--model", "claude-fable-5-2", "--addr", addr,
              "--target-tick", str(a.match_ticks), "--budget-usd", str(a.fable_usd), "--timeout-s", "5400",
              "--out", rel(d / "fable")],
        stdout=open(d / "fable/agent.log", "w"), stderr=subprocess.STDOUT, cwd=ROOT)
    procs["opus"] = subprocess.Popen(
        py + ["tools/agent_claude.py", "--arena", "--model", "claude-opus-5-5", "--addr", addr,
              "--target-tick", str(a.match_ticks), "--budget-usd", str(a.opus_usd), "--timeout-s", "5400",
              "--out", rel(d / "opus")],
        stdout=open(d / "opus/agent.log", "w"), stderr=subprocess.STDOUT, cwd=ROOT)
    # DeepSeek is relaunched if its runner dies before the match ends; it
    # rejoins its slot by name.
    ds = (f"for i in 1 2 3; do {sys.executable} -I tools/agent_ollama.py --arena --provider openai "
          f"--model deepseek-v4-pro --key-file ~/.config/lifeapi/deepseek.env --addr {addr} "
          f"--target-tick {a.match_ticks} --max-turns 200 --out {d / 'deepseek'} && break; "
          f"test -f {d / 'results.json'} && break; echo relaunch $i; done")
    procs["deepseek"] = subprocess.Popen(["bash", "-c", ds], stdout=open(d / "deepseek/agent.log", "a"),
                                         stderr=subprocess.STDOUT, cwd=ROOT)
    for m in a.openai:
        price, responses, base, keyfile, keyname = OPENAI[m]
        oa = (f"for i in 1 2 3; do {sys.executable} -I tools/agent_ollama.py --arena --provider openai "
              f"--base-url {base} --model {m} {'--responses-api ' if responses else ''}"
              f"--key-file ~/.config/lifeapi/{keyfile} --key-name {keyname} --addr {addr} "
              f"--target-tick {a.match_ticks} --max-turns 200 --budget-usd {a.openai_usd} "
              f"--usd-per-mtok {price} --out {d / m} && break; "
              f"test -f {d / 'results.json'} && break; echo relaunch $i; done")
        procs[m] = subprocess.Popen(["bash", "-c", oa], stdout=open(d / m / "agent.log", "a"),
                                    stderr=subprocess.STDOUT, cwd=ROOT)
    procs["bot"] = subprocess.Popen(
        [rel(ROOT / "target/release/lifeapi-bot"), "--arena", "--addr", addr, "--name", "scripted-bot"],
        stdout=open(d / "bot/agent.log", "w"), stderr=subprocess.STDOUT, cwd=ROOT)
    return procs


def finish(procs, match_dir, deadline):
    """Wait for the players to finish (or the deadline), then stop the server."""
    for k in [k for k in procs if k != "server"]:
        left = max(1, deadline - time.time())
        try:
            procs[k].wait(timeout=left)
        except subprocess.TimeoutExpired:
            procs[k].kill()
    procs["server"].terminate()
    try:
        procs["server"].wait(timeout=10)
    except subprocess.TimeoutExpired:
        procs["server"].kill()


def table(out, schedule):
    """Rebuild league.json and LEAGUE.md from every finished match."""
    rating = {p: 1500.0 for p in PLAYERS}
    stats = {p: {"matches": 0, "wins": 0, "rank_sum": 0, "share_sum": 0.0, "cost": 0.0} for p in PLAYERS}
    rows = []
    for m in schedule:
        f = out / m["name"] / "results.json"
        if not f.exists():
            continue
        st = json.loads(f.read_text())["standings"]
        score = {s["name"]: s["score"] for s in st}
        top = max(score.values()) or 1.0
        present = [p for p in PLAYERS if p in score]
        k = 32.0 / (len(present) - 1)  # by THIS match's field (League 2 changed field size mid-league)
        before = dict(rating)
        for a, b in itertools.combinations(present, 2):
            ea = 1 / (1 + 10 ** ((before[b] - before[a]) / 400))
            sa = 1.0 if score[a] > score[b] else 0.0 if score[a] < score[b] else 0.5
            rating[a] += k * (sa - ea)
            rating[b] -= k * (sa - ea)
        for s in st:
            p = s["name"]
            if p not in stats:
                continue
            stats[p]["matches"] += 1
            stats[p]["wins"] += s["rank"] == 1
            stats[p]["rank_sum"] += s["rank"]
            stats[p]["share_sum"] += score[p] / top
        for sub, p in COSTED.items():
            sf = out / m["name"] / sub / "summary.json"
            if sf.exists():
                stats[p]["cost"] += json.loads(sf.read_text()).get("cost_usd") or 0.0
        rows.append({"match": m["name"], "world": m["world"], "seed": m["seed"],
                     "standings": [(s["rank"], s["name"], round(s["score"])) for s in st]})
    ranked = sorted(PLAYERS, key=lambda p: -rating[p])
    league = {"ratings": {p: round(rating[p], 1) for p in ranked}, "stats": stats, "matches": rows}
    (out / "league.json").write_text(json.dumps(league, indent=1))
    md = ["# LifeAPI league", "", f"{len(rows)} of {len(schedule)} matches played. Elo is applied in schedule "
          "order (it depends on order); a player is rated only in matches it played; K = 32/(players in the "
          "match - 1). Mean rank and share mix field sizes when the field changes. Costs: Anthropic as "
          "reported, OpenAI estimated from tokens, DeepSeek not priced.", "",
          "| Rank | Player | Elo | Wins | Mean rank | Mean share of top score | API cost (est. for OpenAI) |",
          "|---|---|---|---|---|---|---|"]
    for i, p in enumerate(ranked, 1):
        s = stats[p]
        n = max(1, s["matches"])
        cost = f"${s['cost']:.2f}" if p in COSTED.values() else "—"
        md.append(f"| {i} | {p} | {rating[p]:.0f} | {s['wins']}/{s['matches']} | {s['rank_sum'] / n:.2f} | "
                  f"{100 * s['share_sum'] / n:.0f}% | {cost} |")
    width = max([len(r["standings"]) for r in rows] or [len(PLAYERS)])
    places = ["1st", "2nd", "3rd"] + [f"{i}th" for i in range(4, width + 1)]
    md += ["", "| Match | World | Seed | " + " | ".join(places[:width]) + " |", "|---|---|---|" + "---|" * width]
    for r in rows:
        cells = [f"{n} ({sc})" for _, n, sc in sorted(r["standings"])]
        md.append(f"| {r['match']} | {r['world']} | {r['seed']} | " + " | ".join(cells) + " |")
    (out / "LEAGUE.md").write_text("\n".join(md) + "\n")
    return md


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--worlds", default="drift,patchy,bloom")
    ap.add_argument("--seeds", default="11,12")
    ap.add_argument("--parallel", type=int, default=3)
    ap.add_argument("--fable-usd", type=float, default=6)
    ap.add_argument("--opus-usd", type=float, default=2)
    ap.add_argument("--openai", action="append", default=[], choices=sorted(OPENAI),
                    help="add an OpenAI player (repeatable)")
    ap.add_argument("--openai-usd", type=float, default=4)
    ap.add_argument("--window-ticks", type=int, default=500)
    ap.add_argument("--window-secs", type=float, default=180)
    ap.add_argument("--first-window-secs", type=float, default=600,
                    help="longer first window: League 1 saw Fable miss window 1 in 6/6 matches at 180 s")
    ap.add_argument("--combat", default="off", choices=["off", "colony", "individual"],
                    help="off (Leagues 1-3), colony (your organisms never fight each other, like ants), "
                         "individual (every organism for itself)")
    ap.add_argument("--evolve-bodies", type=float, default=0.0,
                    help="chance per birth that a child's body mutates (0 = off; e.g. 0.02)")
    ap.add_argument("--realtime", action="store_true",
                    help="continuous clock: the world runs on its own and never waits for a player")
    ap.add_argument("--match-ticks", type=int, default=10000)
    ap.add_argument("--base-port", type=int, default=4400)
    a = ap.parse_args()
    for m in a.openai:
        PLAYERS.append(m)
        COSTED[m] = m
    out = (ROOT / a.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    schedule = [{"name": f"m{i + 1:02d}-{w}-{s}", "world": w, "seed": int(s)}
                for i, (s, w) in enumerate(itertools.product(a.seeds.split(","), a.worlds.split(",")))]
    (out / "schedule.json").write_text(json.dumps(schedule, indent=1))
    # The exact settings, so a league's rules are on record (League 3's were not).
    (out / "args.json").write_text(json.dumps({**vars(a), "argv": sys.argv[1:]}, indent=1))
    todo = [m for m in schedule if not (out / m["name"] / "results.json").exists()]
    for start in range(0, len(todo), a.parallel):
        batch = todo[start:start + a.parallel]
        running = []
        taken = set()
        for j, m in enumerate(batch):
            port = free_pair(a.base_port + 10 * j, taken)
            taken |= {port, port + 1}
            print(f"start {m['name']} on :{port} (viewer :{port + 1})", flush=True)
            running.append((m, launch(out / m["name"], port, port + 1, m["world"], m["seed"], a)))
        deadline = time.time() + 20 * a.window_secs * (a.match_ticks / a.window_ticks / 20) + a.first_window_secs + 900
        for m, procs in running:
            finish(procs, out / m["name"], deadline)
            ok = (out / m["name"] / "results.json").exists()
            print(f"done  {m['name']}: {'results written' if ok else 'NO RESULTS'}", flush=True)
        print("\n".join(table(out, schedule)), flush=True)


if __name__ == "__main__":
    main()

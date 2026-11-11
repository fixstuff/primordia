#!/usr/bin/env python3
"""A Claude model plays LifeAPI through the MCP server, via headless Claude Code.

    python3 tools/agent_claude.py --model claude-opus-5-5 --addr 127.0.0.1:4348 \
        --budget-usd 5 --out runs/<name>

Runs `claude --bare -p` (no hooks, no CLAUDE.md, API-key auth only) with only
the lifeapi MCP server and only its tools allowed, a hard --max-budget-usd
cap, and the same goal text as tools/agent_ollama.py. The API key is read
from --key-file into the child's environment and never printed. Every genome
the model submits is saved and judged by lifeapi-eval at the end.
"""
import argparse, json, os, pathlib, subprocess, sys, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from agent_ollama import saved_genome  # one rule for what an accepted genome was

GOAL = ("YOUR GOAL: have as many of your organisms alive as possible when the world reaches tick {target}. "
        "Work in rounds. Round 1: call catalog, design a genome, submit it, release about 50, subscribe to the "
        "tick 2000 ticks ahead, step. Every later round: call report, read its notes and numbers, then design an "
        "IMPROVED genome that fixes what the report says is limiting you, submit it, release it, and step again. "
        "Give each genome a new name. Always act by calling tools; keep your text short. "
        "Stop when the world reaches tick {target}.")

def load_key(path):
    for line in pathlib.Path(path).read_text().splitlines():
        line = line.strip()
        if line.startswith("ANTHROPIC_API_KEY="):
            return line.split("=", 1)[1].strip().strip('"').strip("'")
    sys.exit(f"no ANTHROPIC_API_KEY line in {path}")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--addr", required=True)
    ap.add_argument("--mcp", default="target/release/lifeapi-mcp")
    ap.add_argument("--eval", default="target/release/lifeapi-eval")
    ap.add_argument("--target-tick", type=int, default=12000)
    ap.add_argument("--budget-usd", type=float, required=True)
    ap.add_argument("--key-file", default=os.path.expanduser("~/.config/controlforge/anthropic.env"))
    ap.add_argument("--timeout-s", type=int, default=3600)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    out = pathlib.Path(a.out).resolve()
    (out / "genomes").mkdir(parents=True, exist_ok=True)
    cfg = {"mcpServers": {"lifeapi": {"command": str(pathlib.Path(a.mcp).resolve()),
                                      "args": ["--addr", a.addr, "--name", a.model]}}}
    (out / "mcp.json").write_text(json.dumps(cfg, indent=1))
    env = dict(os.environ, ANTHROPIC_API_KEY=load_key(a.key_file))
    prompt = GOAL.format(target=a.target_tick) + " The world is at tick 0. Begin."
    cmd = ["claude", "--bare", "-p", prompt, "--model", a.model,
           "--mcp-config", str(out / "mcp.json"), "--strict-mcp-config",
           "--allowedTools", "mcp__lifeapi__*",
           "--disallowedTools", "Bash", "Edit", "Write", "Read", "Glob", "Grep", "WebFetch", "WebSearch",
           "NotebookEdit", "Agent", "Task",
           "--output-format", "stream-json", "--verbose",
           "--max-budget-usd", str(a.budget_usd), "--no-session-persistence"]
    t0 = time.time()
    with open(out / "stream.jsonl", "w") as log:
        p = subprocess.run(cmd, cwd=out, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=a.timeout_s)
    secs = round(time.time() - t0)

    # Pull the tool calls and their results out of the stream.
    uses, results, final = {}, {}, None
    for line in open(out / "stream.jsonl"):
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if ev.get("type") == "assistant":
            for c in ev["message"].get("content", []):
                if c.get("type") == "tool_use":
                    uses[c["id"]] = c
        elif ev.get("type") == "user":
            for c in ev["message"].get("content", []):
                if isinstance(c, dict) and c.get("type") == "tool_result":
                    results[c["tool_use_id"]] = c
        elif ev.get("type") == "result":
            final = ev
    genomes, tick, calls = [], 0, []
    for uid, u in uses.items():
        name = u["name"].removeprefix("mcp__lifeapi__")
        r = results.get(uid, {})
        text = r.get("content")
        if isinstance(text, list):
            text = "".join(x.get("text", "") for x in text if isinstance(x, dict))
        err = bool(r.get("is_error"))
        calls.append(name + ("!" if err else ""))
        try:
            v = json.loads(text or "")
            tick = v.get("tick", tick) if isinstance(v, dict) else tick
        except ValueError:
            v = None
        if name == "submit_genome" and not err:
            k = len(genomes) + 1
            g = saved_genome(u["input"])
            f = out / "genomes" / f"{k:02d}.json"
            f.write_text(json.dumps(g, indent=1))
            genomes.append({"n": k, "file": str(f), "name": g.get("name"), "tick": tick})
    summary = {"model": a.model, "exit": p.returncode, "seconds": secs, "final_tick": tick,
               "tool_calls": calls, "genomes": genomes,
               "cost_usd": final.get("total_cost_usd") if final else None,
               "result_subtype": final.get("subtype") if final else None,
               "num_turns": final.get("num_turns") if final else None,
               "last_text": (final.get("result") or "")[:1500] if final else None}
    if genomes:
        r = subprocess.run([a.eval, *[g["file"] for g in genomes]], capture_output=True, text=True)
        (out / "eval.json").write_text(r.stdout)
        try:
            for g, row in zip(genomes, json.loads(r.stdout)["results"]):
                g["eval_mean_population"] = row.get("mean_population", row.get("error"))
        except ValueError:
            pass
    (out / "summary.json").write_text(json.dumps(summary, indent=1))
    print(json.dumps(summary, indent=1))

if __name__ == "__main__":
    main()

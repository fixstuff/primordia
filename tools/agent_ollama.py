#!/usr/bin/env python3
"""A local LLM plays LifeAPI through the MCP server.

    python3 tools/agent_ollama.py --model qwen3.5:4b --addr 127.0.0.1:4343 \
        --mcp target/release/lifeapi-mcp --eval target/release/lifeapi-eval \
        --target-tick 12000 --out runs/<name>

Standard library only. It starts lifeapi-mcp as a subprocess (MCP over
stdio), turns the MCP tool list into Ollama tools, and loops: model -> tool
calls -> MCP -> results back to the model, until the world reaches the target
tick or the turn limit. Every genome the model submits is saved, and at the
end each one is judged by lifeapi-eval (alone in a fresh pool, fixed seeds),
so "did it improve?" is a number, not the model's say-so.
"""
import argparse, json, pathlib, subprocess, sys, time, urllib.request

ARENA_GOAL = (
    "THIS IS AN ARENA MATCH against other AI players in the same world. Your SCORE is your biomass "
    "(sum of radius^2) counting ONLY organisms BORN IN THE POOL (the founders you release do not count, only "
    "their descendants), AVERAGED OVER EVERY TICK of the match, which ends at tick {target}; the highest score "
    "wins. Only lineages that eat and reproduce score. Founders cost 5 x (radius/5)^2 budget each and start with "
    "r^2 energy (vacuoles start empty). "
    "The server owns the clock: time moves in windows. In each window, make your moves (catalog, report, "
    "submit_genome, release, observe), then call end_turn. end_turn returns when the next window opens, with "
    "the scoreboard (everyone's alive count and biomass, your rank, ticks left, match_over). If you take too "
    "long, the window closes without your moves. step and subscribe are refused. Call catalog first: it "
    "describes this world. Keep playing until end_turn says match_over is true. Always act by calling tools; "
    "keep your text short.")

def mcp_start(binary, addr, name):
    p = subprocess.Popen([binary, "--addr", addr, "--name", name],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    state = {"id": 0}
    def rpc(method, params):
        state["id"] += 1
        p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": state["id"], "method": method, "params": params}) + "\n")
        p.stdin.flush()
        return json.loads(p.stdout.readline())
    init = rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                              "clientInfo": {"name": "agent_ollama", "version": "0.1"}})
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
    p.stdin.flush()
    tools = rpc("tools/list", {})["result"]["tools"]
    return p, rpc, init["result"]["instructions"], tools

def ollama_chat(url, model, messages, tools, think):
    body = {"model": model, "messages": messages, "tools": tools, "stream": False,
            "think": think, "options": {"temperature": 0.4, "num_ctx": 12288}}
    req = urllib.request.Request(url + "/api/chat", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=1800) as r:
        return json.loads(r.read())

def openai_chat(base, key, model, messages, tools):
    """OpenAI-style /chat/completions (DeepSeek and others). Returns a reply
    shaped like Ollama's, so the loop below handles both. Retries transient
    network and 5xx/429 errors with backoff (arena match 2's first attempt
    lost its DeepSeek player to one "connection reset by peer")."""
    body = {"model": model, "messages": messages, "tools": tools}
    if "api.openai.com" not in base:
        body["temperature"] = 0.4  # OpenAI's reasoning models accept only the default
    data = json.dumps(body).encode()
    for attempt in range(6):
        req = urllib.request.Request(base.rstrip("/") + "/chat/completions", data=data,
                                     headers={"Content-Type": "application/json", "Authorization": "Bearer " + key})
        try:
            with urllib.request.urlopen(req, timeout=600) as r:
                d = json.loads(r.read())
            break
        except urllib.error.HTTPError as e:
            if e.code not in (429, 500, 502, 503, 504) or attempt == 5:
                raise
            print(f"  (HTTP {e.code}; retrying in {2 ** attempt}s)", flush=True)
        except (ConnectionError, TimeoutError, urllib.error.URLError, OSError) as e:
            if attempt == 5:
                raise
            print(f"  ({type(e).__name__}; retrying in {2 ** attempt}s)", flush=True)
        time.sleep(2 ** attempt)
    m = d["choices"][0]["message"]
    calls = []
    for c in m.get("tool_calls") or []:
        args = c["function"].get("arguments") or "{}"
        try:
            args = json.loads(args) if isinstance(args, str) else args
        except ValueError:
            args = {}
        calls.append({"id": c.get("id"), "function": {"name": c["function"]["name"], "arguments": args}})
    u = d.get("usage") or {}
    # Sent back next turn as the assistant message (reasoning_content is never sent back).
    raw = {"role": "assistant", "content": m.get("content") or "",
           "tool_calls": m.get("tool_calls") or None}
    return {"message": {"role": "assistant", "content": m.get("content") or "", "tool_calls": calls},
            "raw_message": raw, "eval_count": u.get("completion_tokens"),
            "prompt_eval_count": u.get("prompt_tokens"),
            "cached_tokens": (u.get("prompt_tokens_details") or {}).get("cached_tokens") or 0}

def responses_chat(base, key, model, messages, tools, effort):
    """OpenAI's /v1/responses, for models that take function tools only there
    with reasoning on (gpt-6-astra). The chat-shaped history is converted
    each call (stateless: reasoning items are not carried between turns).
    Returns the same shape as openai_chat."""
    items, instructions = [], None
    for m in messages:
        if m["role"] == "system":
            instructions = m["content"]
        elif m["role"] == "user":
            items.append({"role": "user", "content": m["content"]})
        elif m["role"] == "assistant":
            if m.get("content"):
                items.append({"role": "assistant", "content": m["content"]})
            for c in m.get("tool_calls") or []:
                items.append({"type": "function_call", "call_id": c["id"], "name": c["function"]["name"],
                              "arguments": c["function"]["arguments"]})
        elif m["role"] == "tool":
            items.append({"type": "function_call_output", "call_id": m["tool_call_id"], "output": m["content"]})
    body = {"model": model, "input": items, "instructions": instructions, "store": False,
            "tools": [{"type": "function", "name": t["function"]["name"], "description": t["function"]["description"],
                       "parameters": t["function"]["parameters"]} for t in tools]}
    if effort:
        body["reasoning"] = {"effort": effort}
    data = json.dumps(body).encode()
    for attempt in range(6):
        req = urllib.request.Request(base.rstrip("/") + "/responses", data=data,
                                     headers={"Content-Type": "application/json", "Authorization": "Bearer " + key})
        try:
            with urllib.request.urlopen(req, timeout=900) as r:
                d = json.loads(r.read())
            break
        except urllib.error.HTTPError as e:
            if e.code not in (429, 500, 502, 503, 504) or attempt == 5:
                print(f"  (HTTP {e.code}: {e.read()[:500]!r})", flush=True)
                raise
            print(f"  (HTTP {e.code}; retrying in {2 ** attempt}s)", flush=True)
        except (ConnectionError, TimeoutError, urllib.error.URLError, OSError) as e:
            if attempt == 5:
                raise
            print(f"  ({type(e).__name__}; retrying in {2 ** attempt}s)", flush=True)
        time.sleep(2 ** attempt)
    text, calls, raw_calls = "", [], []
    for it in d.get("output") or []:
        if it.get("type") == "message":
            text += "".join(c.get("text", "") for c in it.get("content") or [] if c.get("type") == "output_text")
        elif it.get("type") == "function_call":
            a = it.get("arguments") or "{}"
            try:
                args = json.loads(a)
            except ValueError:
                args = {}
            calls.append({"id": it["call_id"], "function": {"name": it["name"], "arguments": args}})
            raw_calls.append({"id": it["call_id"], "type": "function",
                              "function": {"name": it["name"], "arguments": a}})
    u = d.get("usage") or {}
    raw = {"role": "assistant", "content": text, "tool_calls": raw_calls or None}
    return {"message": {"role": "assistant", "content": text, "tool_calls": calls}, "raw_message": raw,
            "eval_count": u.get("output_tokens"), "prompt_eval_count": u.get("input_tokens"),
            "cached_tokens": (u.get("input_tokens_details") or {}).get("cached_tokens") or 0}

def saved_genome(args):
    """The genome as the server accepted it: the same repairs lifeapi-mcp
    makes (fields beside `genome` moved in, JSON strings parsed), so the
    saved file is what lifeapi-eval should judge."""
    def unstring(x):
        if isinstance(x, str) and x.strip()[:1] in "[{":
            try: return json.loads(x)
            except ValueError: return x
        return x
    g = unstring(args.get("genome"))
    if isinstance(g, list):
        g = {"body": g}
    if not isinstance(g, dict):
        g = {}
    g = dict(g)
    for f in ("schema", "name", "body", "brain", "mutation"):
        if f in args and f not in g:
            g[f] = args[f]
    for f in ("body", "brain", "mutation"):
        if f in g:
            g[f] = unstring(g[f])
    return g

def load_key(path, name):
    for line in pathlib.Path(path).expanduser().read_text().splitlines():
        line = line.strip()
        if line.startswith(name + "="):
            return line.split("=", 1)[1].strip().strip('"').strip("'")
    sys.exit(f"no {name}= line in {path}")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="qwen3.5:4b")
    ap.add_argument("--ollama", default="http://127.0.0.1:11434")
    ap.add_argument("--addr", default="127.0.0.1:4343")
    ap.add_argument("--mcp", default="target/release/lifeapi-mcp")
    ap.add_argument("--eval", default="target/release/lifeapi-eval")
    ap.add_argument("--target-tick", type=int, default=12000)
    ap.add_argument("--max-turns", type=int, default=60)
    ap.add_argument("--think", action="store_true")
    ap.add_argument("--out", required=True)
    ap.add_argument("--provider", choices=["ollama", "openai"], default="ollama",
                    help="openai = any OpenAI-style /chat/completions API, e.g. DeepSeek")
    ap.add_argument("--base-url", default="https://api.deepseek.com/v1")
    ap.add_argument("--key-file", help="file with a NAME=value line (never printed)")
    ap.add_argument("--key-name", default="DEEPSEEK_API_KEY")
    ap.add_argument("--arena", action="store_true", help="play an arena match (end_turn) until match_over")
    ap.add_argument("--responses-api", action="store_true",
                    help="use OpenAI's /v1/responses (gpt-6-astra takes tools with reasoning only there)")
    ap.add_argument("--reasoning-effort", help="with --responses-api: low | medium | high")
    ap.add_argument("--budget-usd", type=float, help="stop once the estimated spend reaches this")
    ap.add_argument("--usd-per-mtok", default="0,0,0",
                    help="input,cached input,output USD per million tokens, for the estimate")
    a = ap.parse_args()
    key = load_key(a.key_file, a.key_name) if a.provider == "openai" else None

    out = pathlib.Path(a.out)
    (out / "genomes").mkdir(parents=True, exist_ok=True)
    log = open(out / "transcript.jsonl", "a")
    def note(kind, **kw):
        kw.update(kind=kind, t=round(time.time(), 1))
        log.write(json.dumps(kw) + "\n"); log.flush()

    proc, rpc, instructions, mcp_tools = mcp_start(a.mcp, a.addr, a.model)
    tools = [{"type": "function", "function": {"name": t["name"], "description": t["description"],
                                               "parameters": t["inputSchema"]}} for t in mcp_tools]
    system = instructions + "\n\n" + (ARENA_GOAL.format(target=a.target_tick) if a.arena else
              f"YOUR GOAL: have as many of your organisms alive as possible when the world reaches tick "
              f"{a.target_tick}. Work in rounds. Round 1: call catalog, design a genome, submit it, release "
              "about 50, subscribe to the tick 2000 ticks ahead, step. Every later round: call report, read its "
              "notes and numbers, then design an IMPROVED genome that fixes what the report says is limiting "
              "you, submit it, release it, and step again. Give each genome a new name. Always act by "
              "calling tools; keep your text short.")
    messages = [{"role": "system", "content": system},
                {"role": "user", "content": "Begin. The world is at tick 0."}]
    note("start", model=a.model, target=a.target_tick, system=system)

    tick, genomes, t0, match_over = 0, [], time.time(), False
    p_in, p_cached, p_out = (float(x) for x in a.usd_per_mtok.split(","))
    usage = {"prompt_tokens": 0, "cached_tokens": 0, "completion_tokens": 0, "cost_usd": 0.0}
    for turn in range(1, a.max_turns + 1):
        if a.budget_usd is not None and usage["cost_usd"] >= a.budget_usd:
            print(f"budget reached: ${usage['cost_usd']:.2f} of ${a.budget_usd:.2f}", flush=True)
            note("budget", **usage)
            break
        if a.provider == "openai":
            resp = (responses_chat(a.base_url, key, a.model, messages, tools, a.reasoning_effort)
                    if a.responses_api else openai_chat(a.base_url, key, a.model, messages, tools))
            pt, ct, ca = resp.get("prompt_eval_count") or 0, resp.get("eval_count") or 0, resp.get("cached_tokens", 0)
            usage["prompt_tokens"] += pt
            usage["cached_tokens"] += ca
            usage["completion_tokens"] += ct
            usage["cost_usd"] += ((pt - ca) * p_in + ca * p_cached + ct * p_out) / 1e6
            (out / "summary.json").write_text(json.dumps({"model": a.model, **usage}, indent=1))
            messages.append({k: v for k, v in resp["raw_message"].items() if v is not None})
        else:
            resp = ollama_chat(a.ollama, a.model, messages, tools, a.think)
            messages.append({k: v for k, v in resp["message"].items() if k in ("role", "content", "tool_calls")})
        msg = resp["message"]
        calls = msg.get("tool_calls") or []
        note("model", turn=turn, content=msg.get("content", ""), calls=calls,
             eval_tokens=resp.get("eval_count"), prompt_tokens=resp.get("prompt_eval_count"))
        print(f"[turn {turn:>2} | tick {tick:>6} | {time.time()-t0:6.0f}s] "
              f"{(msg.get('content') or '').strip()[:100]!r} calls={[c['function']['name'] for c in calls]}",
              flush=True)
        if not calls:
            if "DONE" in (msg.get("content") or "") and tick >= a.target_tick:
                break
            messages.append({"role": "user", "content":
                             f"Keep going: act by calling a tool. The world is at tick {tick} of {a.target_tick}."})
            continue
        for c in calls:
            name, args = c["function"]["name"], c["function"].get("arguments") or {}
            if isinstance(args, str):
                try: args = json.loads(args)
                except ValueError: args = {}
            r = rpc("tools/call", {"name": name, "arguments": args})["result"]
            text, is_err = r["content"][0]["text"], r["isError"]
            note("tool", name=name, args=args, error=is_err, result=text[:4000])
            try:
                v = json.loads(text)
                tick = v.get("tick", tick) if isinstance(v, dict) else tick
                if isinstance(v, dict) and v.get("match_over"):
                    match_over = True
            except ValueError:
                v = None
            if name == "submit_genome" and not is_err:
                k = len(genomes) + 1
                g = saved_genome(args)
                f = out / "genomes" / f"{k:02d}.json"
                f.write_text(json.dumps(g, indent=1))
                genomes.append({"n": k, "file": str(f), "name": g.get("name"),
                                "species": v.get("species") if v else None, "tick": tick})
            shown = text if len(text) < 3000 else text[:3000] + " …(truncated)"
            tm = {"role": "tool", "content": ("ERROR: " if is_err else "") + shown}
            if a.provider == "openai":
                tm["tool_call_id"] = c.get("id")
            else:
                tm["tool_name"] = name
            messages.append(tm)
        # Keep the context bounded: the system prompt plus the most recent
        # exchanges, cut so the kept part starts at an assistant message
        # (OpenAI-style APIs refuse a tool result whose call was dropped).
        if len(messages) > 40:
            tail = messages[-30:]
            while tail and tail[0]["role"] == "tool":
                tail = tail[1:]
            messages = messages[:2] + tail
        if match_over or (not a.arena and tick >= a.target_tick):
            break
    proc.kill()
    note("end", tick=tick, turns=turn, genomes=genomes, seconds=round(time.time() - t0))
    print(f"\nended at tick {tick} after {turn} turns, {len(genomes)} genomes submitted")
    (out / "genomes.json").write_text(json.dumps(genomes, indent=1))
    if genomes:
        r = subprocess.run([a.eval, *[g["file"] for g in genomes]], capture_output=True, text=True)
        (out / "eval.json").write_text(r.stdout)
        print("\nlifeapi-eval (each genome alone, fixed seeds):")
        try:
            for g, row in zip(genomes, json.loads(r.stdout)["results"]):
                print(f"  #{g['n']:>2} {str(g['name'])[:24]:<24} submitted at tick {g['tick']:>6}: "
                      f"{row.get('mean_population', row.get('error'))}")
        except ValueError:
            print(r.stdout, r.stderr)

if __name__ == "__main__":
    main()

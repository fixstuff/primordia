#!/usr/bin/env python3
"""LifeAPI ratings with error bars, and how they line up with benchmarks.

    python3 -I tools/analysis.py                      # every league in runs/
    python3 -I tools/analysis.py --leagues league3,league4 --world drift
    python3 -I tools/analysis.py --benchmarks tools/benchmarks.json --out runs/ANALYSIS.md

Ratings: Bradley-Terry, fitted to every pair in every match (the higher
score wins the pair, equal scores split it). Unlike the league Elo, it
does not depend on the order matches were played in. It is shown on an
Elo-like scale (400 * log10 strength, mean 1500). A weak prior (one draw
against an average player) keeps a player that never wins finite.

Error bars: the matches are resampled with replacement (bootstrap), the
ratings refitted each time, and the middle 95% of each rating reported.

Benchmarks: a JSON file {"model": {"benchmark": score, ...}, ...} (higher
= better). For each benchmark with at least 3 rated models, the Spearman
rank correlation and Pearson correlation with the LifeAPI rating, an exact
permutation p-value for Spearman (n <= 8), and a 95% interval for Spearman
that carries the LifeAPI rating's own uncertainty (recomputed on every
bootstrap sample). With only a handful of models, read these as direction,
not proof.

Standard library only.
"""
import argparse, itertools, json, math, pathlib, random, statistics, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_matches(runs, leagues, world, combat, exclude):
    """Every finished league match: (league, match, world, combat, {player: score})."""
    out = []
    for ld in sorted(runs.glob("*league*")):
        if not (ld / "schedule.json").exists():
            continue
        if leagues and not any(k in ld.name for k in leagues):
            continue
        args = {}
        if (ld / "args.json").exists():
            args = json.loads((ld / "args.json").read_text())
        for m in json.loads((ld / "schedule.json").read_text()):
            f = ld / m["name"] / "results.json"
            if not f.exists():
                continue
            r = json.loads(f.read_text())
            mode = args.get("combat") or ("colony" if "colony" in ld.name else "individual" if "individual" in ld.name else "off")
            if world and m["world"] != world:
                continue
            if combat and mode != combat:
                continue
            scores = {s["name"]: s["score"] for s in r["standings"] if s["name"] not in exclude}
            if len(scores) >= 2:
                out.append({"league": ld.name, "match": m["name"], "world": m["world"], "combat": mode, "scores": scores})
    return out


def pairs(matches):
    """(a, b, result for a) for every pair in every match."""
    g = []
    for m in matches:
        sc = m["scores"]
        for a, b in itertools.combinations(sorted(sc), 2):
            g.append((a, b, 1.0 if sc[a] > sc[b] else 0.0 if sc[a] < sc[b] else 0.5))
    return g


def bradley_terry(games, players, iters=500):
    """MM fit (Hunter 2004) with one prior draw per player against a virtual
    average player. Returns {player: Elo-like rating}."""
    p = {x: 1.0 for x in players}
    wins = {x: 0.5 for x in players}  # the prior draw
    for a, b, r in games:
        wins[a] += r
        wins[b] += 1 - r
    for _ in range(iters):
        denom = {x: 1.0 / (p[x] + 1.0) for x in players}  # prior game vs strength 1
        for a, b, _ in games:
            s = 1.0 / (p[a] + p[b])
            denom[a] += s
            denom[b] += s
        new = {x: wins[x] / denom[x] for x in players}
        gm = math.exp(statistics.fmean(math.log(v) for v in new.values()))
        new = {x: v / gm for x, v in new.items()}
        if max(abs(new[x] - p[x]) for x in players) < 1e-10:
            p = new
            break
        p = new
    return {x: 1500 + 400 * math.log10(p[x]) for x in players}


def ranks(xs):
    order = sorted(range(len(xs)), key=lambda i: xs[i])
    r = [0.0] * len(xs)
    i = 0
    while i < len(xs):
        j = i
        while j + 1 < len(xs) and xs[order[j + 1]] == xs[order[i]]:
            j += 1
        for k in range(i, j + 1):
            r[order[k]] = (i + j) / 2 + 1
        i = j + 1
    return r


def pearson(x, y):
    mx, my = statistics.fmean(x), statistics.fmean(y)
    sx = math.sqrt(sum((a - mx) ** 2 for a in x))
    sy = math.sqrt(sum((b - my) ** 2 for b in y))
    if sx == 0 or sy == 0:
        return float("nan")
    return sum((a - mx) * (b - my) for a, b in zip(x, y)) / (sx * sy)


def spearman(x, y):
    return pearson(ranks(x), ranks(y))


def perm_p(x, y):
    """Exact two-sided permutation p-value for Spearman (small n only)."""
    obs = abs(spearman(x, y))
    if len(x) > 8:
        return None
    hits = total = 0
    for perm in itertools.permutations(y):
        total += 1
        hits += abs(spearman(x, list(perm))) >= obs - 1e-12
    return hits / total


def pct(xs, q):
    xs = sorted(xs)
    k = (len(xs) - 1) * q
    lo, hi = math.floor(k), math.ceil(k)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", default="runs")
    ap.add_argument("--leagues", default="", help="comma list of substrings, e.g. league3,league4")
    ap.add_argument("--world", default="")
    ap.add_argument("--combat", default="", choices=["", "off", "colony", "individual"])
    ap.add_argument("--exclude", default="", help="comma list of players to leave out")
    ap.add_argument("--benchmarks", default="", help="JSON {model: {benchmark: score}}")
    ap.add_argument("--boot", type=int, default=1000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    rng = random.Random(a.seed)
    leagues = [x for x in a.leagues.split(",") if x]
    exclude = {x for x in a.exclude.split(",") if x}
    matches = load_matches(ROOT / a.runs, leagues, a.world, a.combat, exclude)
    if not matches:
        sys.exit("no finished league matches match those filters")
    players = sorted({p for m in matches for p in m["scores"]})
    played = {x: sum(x in m["scores"] for m in matches) for x in players}
    rating = bradley_terry(pairs(matches), players)
    boots = {x: [] for x in players}
    boot_ratings = []
    for _ in range(a.boot):
        sample = [rng.choice(matches) for _ in matches]
        present = sorted({p for m in sample for p in m["scores"]})
        r = bradley_terry(pairs(sample), present, iters=200)
        boot_ratings.append(r)
        for x in present:
            boots[x].append(r[x])
    ci = {x: (pct(v, 0.025), pct(v, 0.975)) if v else (float("nan"),) * 2 for x, v in boots.items()}
    order = sorted(players, key=lambda x: -rating[x])
    # How often each player is rated first across the bootstrap samples.
    first = {x: 0 for x in players}
    for r in boot_ratings:
        first[max(r, key=r.get)] += 1

    filt = ", ".join(f for f in [a.leagues and f"leagues {a.leagues}", a.world and f"world {a.world}",
                                 a.combat and f"combat {a.combat}", exclude and f"without {', '.join(sorted(exclude))}"] if f)
    md = ["# LifeAPI ratings and benchmarks", "",
          f"{len(matches)} matches from {len({m['league'] for m in matches})} leagues"
          + (f" ({filt})" if filt else "") + f"; {a.boot} bootstrap resamples of the matches.", "",
          "Bradley-Terry rating (order-free; Elo-like scale). 95% interval from resampling matches. "
          "\"Rated #1\" = share of resamples in which the player comes out on top.", "",
          "| Rank | Player | Rating | 95% interval | Rated #1 | Matches |", "|---|---|---|---|---|---|"]
    for i, x in enumerate(order, 1):
        lo, hi = ci[x]
        md.append(f"| {i} | {x} | {rating[x]:.0f} | {lo:.0f}–{hi:.0f} | {100 * first[x] / a.boot:.0f}% | {played[x]} |")
    # Pairwise: the share of resamples in which the row player out-rates the column player.
    md += ["", "Share of resamples in which the row player is rated above the column player:", "",
           "| | " + " | ".join(order) + " |", "|---|" + "---|" * len(order)]
    for x in order:
        cells = []
        for y in order:
            if x == y:
                cells.append("—")
                continue
            both = [r for r in boot_ratings if x in r and y in r]
            cells.append(f"{100 * sum(r[x] > r[y] for r in both) / max(1, len(both)):.0f}%")
        md.append(f"| {x} | " + " | ".join(cells) + " |")

    result = {"matches": len(matches), "filters": filt, "ratings": {x: {"rating": rating[x], "ci95": ci[x],
              "rated_first": first[x] / a.boot, "matches": played[x]} for x in order}, "benchmarks": {}}

    if a.benchmarks:
        bench = json.loads(pathlib.Path(a.benchmarks).read_text())
        names = sorted({b for k, v in bench.items() if not k.startswith("_") and isinstance(v, dict)
                        for b in v if not b.startswith("_")})
        md += ["", "## Against benchmarks", "",
               "Spearman ρ and Pearson r between the LifeAPI rating and each benchmark, over the models that have "
               "both. p = exact permutation p-value for ρ. ρ interval = 95% over the bootstrap (it carries the "
               "LifeAPI rating's uncertainty, not the benchmark's).", "",
               "| Benchmark | Models | Spearman ρ | ρ 95% interval | p | Pearson r |", "|---|---|---|---|---|---|"]
        for b in names:
            ms = [x for x in order if x in bench and b in bench[x] and isinstance(bench[x][b], (int, float))]
            if len(ms) < 3:
                md.append(f"| {b} | {len(ms)} | — | — | — | — |")
                continue
            xs, ys = [rating[x] for x in ms], [float(bench[x][b]) for x in ms]
            rho, r = spearman(xs, ys), pearson(xs, ys)
            p = perm_p(xs, ys)
            bs = [spearman([br[x] for x in ms], ys) for br in boot_ratings if all(x in br for x in ms)]
            lo, hi = (pct(bs, 0.025), pct(bs, 0.975)) if bs else (float("nan"),) * 2
            md.append(f"| {b} | {len(ms)} | {rho:+.2f} | {lo:+.2f} to {hi:+.2f} | "
                      f"{'—' if p is None else f'{p:.2f}'} | {r:+.2f} |")
            result["benchmarks"][b] = {"models": ms, "spearman": rho, "spearman_ci95": [lo, hi], "perm_p": p, "pearson": r}
        src = bench.get("_sources")
        if src:
            md += ["", "Benchmark sources: " + str(src)]

    text = "\n".join(md) + "\n"
    print(text)
    if a.out:
        pathlib.Path(a.out).write_text(text)
        pathlib.Path(a.out).with_suffix(".json").write_text(json.dumps(result, indent=1))


if __name__ == "__main__":
    main()

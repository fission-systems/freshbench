#!/usr/bin/env python3
"""Ranking uncertainty and per-project spread (reads report.py's inputs).

usage: analysis.py <results_root> <out.json>
  * per-project Union % per tool (GED, type_match and, when scored, byte_match)
  * project bootstrap (10,000 resamples of the projects, seed 0): 95% interval
    of each tool's Union %, and of the Union difference for every tool pair, plus
    the share of resamples in which each ranking position holds
"""
import json
import random
import sys
from collections import defaultdict
from pathlib import Path

root = Path(sys.argv[1])
by = defaultdict(lambda: defaultdict(lambda: [0, 0, 0, 0]))  # proj -> tool -> [rows, union, gedperf, ged_dist]
rows = defaultdict(dict)
for p in root.glob("*/*/*/rows.jsonl"):
    for l in p.read_text().splitlines():
        r = json.loads(l)
        rows[(r["project"], r["compiler"], r["config"], r["binary"], r["function"])][r["tool"]] = r
tools = sorted({t for v in rows.values() for t in v})
for k, per in rows.items():
    if len(per) != len(tools):
        continue
    for t, r in per.items():
        a = by[k[0]][t]
        a[0] += 1
        a[1] += bool(r["ged_perfect"]) or bool(r["type_match_perfect"]) or bool(r.get("byte_match_perfect"))
        a[2] += bool(r["ged_perfect"])
projects = sorted(by)
per_project = {p: {t: round(100 * by[p][t][1] / by[p][t][0], 2) for t in tools} for p in projects}


def union_pct(sample):
    return {t: 100 * sum(by[p][t][1] for p in sample) / sum(by[p][t][0] for p in sample) for t in tools}


rng = random.Random(0)
N = 10000
samples = [union_pct([rng.choice(projects) for _ in projects]) for _ in range(N)]
point = union_pct(projects)
ci = {t: (sorted(s[t] for s in samples)[int(0.025 * N)], sorted(s[t] for s in samples)[int(0.975 * N)]) for t in tools}
pairs = {}
for a in tools:
    for b in tools:
        if a < b:
            d = sorted(s[a] - s[b] for s in samples)
            pairs[f"{a}-{b}"] = {"point": point[a] - point[b], "ci95": (d[int(0.025 * N)], d[int(0.975 * N)]),
                                 "p_a_greater": sum(x > 0 for x in d) / N}
rank_freq = defaultdict(lambda: defaultdict(int))
for s in samples:
    for i, t in enumerate(sorted(tools, key=lambda t: -s[t])):
        rank_freq[t][i + 1] += 1
out = {"tools": tools, "projects": projects, "union_pct": point, "union_ci95": ci, "pairs": pairs,
       "rank_freq": {t: {k: v / N for k, v in sorted(r.items())} for t, r in rank_freq.items()},
       "per_project_union_pct": per_project}
Path(sys.argv[2]).write_text(json.dumps(out, indent=1))
for t in sorted(tools, key=lambda t: -point[t]):
    print(f"{t:8s} Union {point[t]:.2f}%  95% CI [{ci[t][0]:.2f}, {ci[t][1]:.2f}]  rank freq {dict(out['rank_freq'][t])}")
for k, v in pairs.items():
    print(k, f"{v['point']:+.2f} pp CI [{v['ci95'][0]:+.2f}, {v['ci95'][1]:+.2f}] P(>0)={v['p_a_greater']:.3f}")

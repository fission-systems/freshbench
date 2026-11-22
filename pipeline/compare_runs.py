#!/usr/bin/env python3
"""Before/after for one tool on FreshBench: output hashes first, scores only where output changed.

usage: compare_runs.py <base_results_root> <new_results_root> [--tool fission] [--ref-tool kuna]
roots: <slug>/<build>/<tool>/rows.jsonl. GED distance is summed on rows where the
tool has a GED in BOTH runs (and, for the reference line, where the ref tool has one).
"""
import argparse, json, collections
from pathlib import Path
ap = argparse.ArgumentParser(); ap.add_argument("base"); ap.add_argument("new")
ap.add_argument("--tool", default="fission"); ap.add_argument("--list", type=int, default=15)
a = ap.parse_args()
def load(root, tool):
    out = {}
    for p in Path(root).glob(f"*/*/{tool}/rows.jsonl"):
        for l in p.read_text().splitlines():
            r = json.loads(l); out[(r["project"], r["compiler"], r["config"], r["binary"], r["function"])] = r
    return out
B, N = load(a.base, a.tool), load(a.new, a.tool)
keys = sorted(set(B) & set(N))
print(f"rows base {len(B)} new {len(N)} common {len(keys)}")
changed = [k for k in keys if B[k]["code_sha256"] != N[k]["code_sha256"]]
print(f"output changed: {len(changed)} rows ({100*len(changed)/max(1,len(keys)):.2f}%)")
st = collections.Counter()
for k in keys:
    b, n = B[k], N[k]
    if b["ged"] is not None and n["ged"] is not None:
        st["both_ged"] += 1; st["dist_base"] += b["ged"]; st["dist_new"] += n["ged"]
        if k not in changed and b["ged"] != n["ged"]: st["ged_moved_same_text"] += 1
    st["ged_perfect_base"] += bool(b["ged_perfect"]); st["ged_perfect_new"] += bool(n["ged_perfect"])
    st["union_base"] += bool(b["ged_perfect"]) or bool(b["type_match_perfect"])
    st["union_new"] += bool(n["ged_perfect"]) or bool(n["type_match_perfect"])
    st["type_perfect_base"] += bool(b["type_match_perfect"]); st["type_perfect_new"] += bool(n["type_match_perfect"])
    st["decompiled_base"] += b["decompiled"]; st["decompiled_new"] += n["decompiled"]
for x in ("ged_perfect", "union", "type_perfect", "decompiled"):
    print(f"{x:13s} {st[x+'_base']:6d} -> {st[x+'_new']:6d} ({st[x+'_new']-st[x+'_base']:+d})")
print(f"GED distance on {st['both_ged']} rows: {st['dist_base']:.0f} -> {st['dist_new']:.0f} ({st['dist_new']-st['dist_base']:+.0f}, {100*(st['dist_new']/max(1,st['dist_base'])-1):+.2f}%)")
print(f"GED moved with identical output (scorer noise): {st['ged_moved_same_text']}")
imp = sorted(((N[k]["ged"] - B[k]["ged"], k) for k in changed if B[k]["ged"] is not None and N[k]["ged"] is not None))
better = sum(1 for d, _ in imp if d < 0); worse = sum(1 for d, _ in imp if d > 0)
print(f"changed rows with GED: improved {better}, regressed {worse}, same {len(imp)-better-worse}")
lost = [k for k in keys if B[k]["ged_perfect"] and not N[k]["ged_perfect"]]
print(f"perfect lost: {len(lost)}  gained: {sum(1 for k in keys if N[k]['ged_perfect'] and not B[k]['ged_perfect'])}")
for d, k in imp[:a.list]: print(f"  {d:+.0f}  {'/'.join(k)}")
for d, k in imp[-a.list:][::-1]:
    if d > 0: print(f"  {d:+.0f}  {'/'.join(k)}")

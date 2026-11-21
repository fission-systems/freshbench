#!/usr/bin/env python3
"""Aggregate rows.jsonl from every (project, build, tool) into ranking tables.

usage: report.py <results_root> <out_dir>
<results_root>/<project>/<compiler>-<config>/<tool>/{rows.jsonl,run.json}

Per tool and per slice (overall, per compiler, per config, per build):
  rows           selected functions in the slice (same set for every tool)
  decompiled     functions with non-empty output
  GED perfect    GED == 0;  GED scored = rows with a GED value
  GED distance   sum of GED over the COMMON rows (every tool has a GED value),
                 reported next to the perfect count; also each tool's own sum
  type perfect / scored / mean   DecBench type_match (1.0 = perfect)
  Union          rows perfect on GED or on type_match (byte_match not run),
                 over all rows
"""
from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path

root, outd = Path(sys.argv[1]), Path(sys.argv[2])
outd.mkdir(parents=True, exist_ok=True)
rows = []
runs = []
for p in sorted(root.glob("*/*/*/rows.jsonl")):
    rows.extend(json.loads(l) for l in p.read_text().splitlines() if l.strip())
    rj = p.parent / "run.json"
    if rj.exists():
        runs.append(json.loads(rj.read_text()))
tools = sorted({r["tool"] for r in rows})
key = lambda r: (r["project"], r["compiler"], r["config"], r["binary"], r["function"])
by_row = defaultdict(dict)
for r in rows:
    by_row[key(r)][r["tool"]] = r
complete = {k: v for k, v in by_row.items() if all(t in v for t in tools)}
missing = len(by_row) - len(complete)


def slices(k):
    proj, comp, conf = k[0], k[1], k[2]
    return ["all", f"compiler={comp}", f"config={conf}", f"build={comp}-{conf}", f"project={proj}"]


agg = defaultdict(lambda: defaultdict(lambda: defaultdict(float)))
for k, per in complete.items():
    common = all(per[t]["ged"] is not None for t in tools)
    for s in slices(k):
        for t in tools:
            r = per[t]
            a = agg[s][t]
            a["rows"] += 1
            a["decompiled"] += r["decompiled"]
            if r["ged"] is not None:
                a["ged_scored"] += 1
                a["ged_sum_own"] += r["ged"]
                a["ged_perfect"] += r["ged"] == 0
            if common:
                a["common_rows"] += 1
                a["ged_distance_common"] += r["ged"]
            if r["type_match"] is not None:
                a["type_scored"] += 1
                a["type_sum"] += r["type_match"]
                a["type_perfect"] += bool(r["type_match_perfect"])
            a["union"] += bool(r["ged_perfect"]) or bool(r["type_match_perfect"])

summary = {}
for s, per in agg.items():
    summary[s] = {}
    for t, a in per.items():
        d = dict(a)
        d["type_mean"] = d.get("type_sum", 0) / d["type_scored"] if d.get("type_scored") else None
        d["union_pct"] = 100 * d.get("union", 0) / d["rows"] if d["rows"] else None
        summary[s][t] = {k: (int(v) if isinstance(v, float) and v.is_integer() else v) for k, v in d.items()}

# failures / timing per tool
fail = defaultdict(lambda: defaultdict(float))
for rj in runs:
    t = rj["tool"]
    for b, info in rj["binaries"].items():
        f = fail[t]
        f["binaries"] += 1
        f["wall_s"] += info["wall_s"]
        f["cpu_s"] += info["cpu_s"]
        f["functions"] += info["functions"]
        f["decompiled"] += info["decompiled"]
        f["maxrss_kb"] = max(f["maxrss_kb"], info.get("children_maxrss_kb") or 0)
        fl = info.get("failure") or ""
        if fl.startswith("timeout"):
            f["binary_timeouts"] += 1
        elif fl.startswith("memory"):
            f["binary_oom"] += 1
        elif fl:
            f["binary_errors"] += 1
        if info.get("timed_out"):
            f["timed_out_flag"] += 1
(outd / "summary.json").write_text(json.dumps({"tools": tools, "complete_rows": len(complete),
                                               "rows_missing_a_tool": missing, "slices": summary,
                                               "runs": {t: dict(v) for t, v in fail.items()}}, indent=1))


def table(slice_names, title):
    out = [f"### {title}", "",
           "| slice | tool | rows | decompiled | GED perfect | GED scored | GED distance (common rows) | type perfect / scored (mean) | Union(GED,type) |",
           "|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for s in slice_names:
        if s not in summary:
            continue
        ranked = sorted(summary[s].items(), key=lambda kv: (-kv[1].get("union", 0), kv[1].get("ged_distance_common", 0)))
        for t, d in ranked:
            tm = f"{d['type_mean']:.3f}" if d.get("type_mean") is not None else "-"
            out.append(f"| {s} | {t} | {d['rows']} | {d.get('decompiled', 0)} | {d.get('ged_perfect', 0)} | "
                       f"{d.get('ged_scored', 0)} | {d.get('ged_distance_common', 0)} ({d.get('common_rows', 0)}) | "
                       f"{d.get('type_perfect', 0)} / {d.get('type_scored', 0)} ({tm}) | "
                       f"{d.get('union', 0)} ({d['union_pct']:.1f}%) |")
    return "\n".join(out) + "\n"


confs = ["O0", "O1", "O2", "O3", "Os", "O2-noinline"]
md = ["# FreshBench results", "",
      f"Rows scored for every tool: {len(complete)} (rows missing a tool: {missing}).", "",
      table(["all", "compiler=gcc", "compiler=clang"], "Overall"),
      table([f"config={c}" for c in confs], "Per optimization level (both compilers)"),
      table([f"build={c}-{o}" for c in ("gcc", "clang") for o in confs], "Per build"),
      "### Runs", "", "| tool | binaries | functions | decompiled | wall s | CPU s | peak RSS MB | binary timeouts | OOM | errors |",
      "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
for t, f in sorted(fail.items()):
    md.append(f"| {t} | {int(f['binaries'])} | {int(f['functions'])} | {int(f['decompiled'])} | {f['wall_s']:.0f} | "
              f"{f['cpu_s']:.0f} | {f['maxrss_kb'] / 1024:.0f} | {int(f['binary_timeouts'])} | {int(f['binary_oom'])} | "
              f"{int(f['binary_errors'])} |")
(outd / "summary.md").write_text("\n".join(md) + "\n")
print("\n".join(md[:12]))

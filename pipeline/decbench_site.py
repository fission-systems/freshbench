#!/usr/bin/env python3
"""FreshBench results -> DecBench's own FunctionData, aggregates and static site.

usage: decbench_site.py <results_root> <out_dir> [--site]

Every number DecBench's leaderboard and data pages show is computed by DecBench's
own code (decbench.rendering.aggregate) from a FunctionData built here, so the
fairness rules are DecBench's, not a re-implementation:
  * per-metric denominators shared by all tools; a tool that failed on a
    measurable function counts as a miss;
  * Union = perfect on >= 1 measurable metric;
  * errors = functions a tool produced no output for;
  * compiles = byte_match recompiled the output (fix-up pass);
  * distance = GED value / type fp+fn / byte_match changed_lines (mean, median, at 0);
  * Joern health = source CFG lost / output not parsed;
  * cost = batch wall time / function count, per binary (median and mean).
Writes <out_dir>/function_results.json, aggregates.json, dataset.json, summary.md
and, with --site, a DecBench-style static site under <out_dir>/site.
"""
from __future__ import annotations

import json
import statistics
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path

from decbench.models.function_data import BinaryGroup, DatasetPreset, FunctionData, FunctionRecord
from decbench.models.scoreboard import Scoreboard
from decbench.rendering.aggregate import build_aggregates, build_dataset_page

root, outd = Path(sys.argv[1]), Path(sys.argv[2])
outd.mkdir(parents=True, exist_ok=True)
TOOLS = ["kuna", "angr", "fission", "ghidra"]
VERSIONS = {"kuna": "1.1003", "angr": "10.0.7", "fission": "18f9e3d", "ghidra": "12.2"}
OPTS = ["O0", "O1", "O2", "O3", "Os", "O2-noinline"]
METRICS = ["ged", "type_match", "byte_match"]
PERFECT = {"ged": 0.0, "type_match": 1.0, "byte_match": 1.0}

groups: dict[tuple, dict[str, FunctionRecord]] = defaultdict(dict)
cost = defaultdict(list)  # tool -> per-binary seconds per function
cost_n = defaultdict(int)
have_bm = False
for rows_path in sorted(root.glob("*/*/*/rows.jsonl")):
    run = json.loads((rows_path.parent / "run.json").read_text())
    tool = run["tool"]
    for b in run["binaries"].values():
        if b.get("decompiled"):
            cost[tool].append(b["wall_s"] / max(1, b["decompiled"]))
            cost_n[tool] += b["decompiled"]
    for line in rows_path.read_text().splitlines():
        r = json.loads(line)
        build = f"{r['compiler']}-{r['config']}"
        g = groups[(r["project"], build, r["binary"])]
        rec = g.get(r["function"])
        if rec is None:
            rec = g[r["function"]] = FunctionRecord(
                function=r["function"],
                datasets=["full", r["compiler"], r["config"], build])
        rec.decompiled[tool] = bool(r["decompiled"])
        vals, perf, dist = {}, {}, {}
        if r.get("ged") is not None:
            vals["ged"] = r["ged"]; perf["ged"] = r["ged"] == 0; dist["ged"] = r["ged"]
        if r.get("type_match") is not None:
            vals["type_match"] = r["type_match"]; perf["type_match"] = bool(r["type_match_perfect"])
            md = r.get("type_match_meta") or {}
            if "fp" in md and "fn" in md:
                dist["type_match"] = float(md["fp"]) + float(md["fn"])
        if r.get("byte_match") is not None:
            have_bm = True
            vals["byte_match"] = r["byte_match"]; perf["byte_match"] = bool(r["byte_match_perfect"])
            md = r.get("byte_match_meta") or {}
            if md.get("changed_lines") is not None:
                dist["byte_match"] = float(md["changed_lines"])
            if "compilable" in md:
                rec.compiles[tool] = bool(md["compilable"])
        if vals:
            rec.values[tool] = vals; rec.perfects[tool] = perf
        if dist:
            rec.distances[tool] = dist

metrics = METRICS if have_bm else METRICS[:2]
presets = [DatasetPreset(name="full", label="full", description="every build")]
presets += [DatasetPreset(name=c, label=c, description=f"{c} builds") for c in ("gcc", "clang")]
presets += [DatasetPreset(name=o, label=o, description=f"-{o} (GCC and Clang)") for o in OPTS]
presets += [DatasetPreset(name=f"{c}-{o}", label=f"{c}-{o}", description=f"{c} -{o}")
            for c in ("gcc", "clang") for o in OPTS]
fd = FunctionData(
    decompilers=TOOLS, decompiler_versions=VERSIONS, metrics=metrics,
    perfect_values={m: PERFECT[m] for m in metrics},
    groups=[BinaryGroup(project=p, opt_level=b, binary=n, arch="x86-64", functions=list(fs.values()))
            for (p, b, n), fs in sorted(groups.items())],
    dataset_presets=presets,
    cost_info={"decompile_time": {
        t: {"per_fn_mean_s": statistics.mean(v), "per_fn_median_s": statistics.median(v),
            "functions": cost_n[t], "binaries": len(v)} for t, v in cost.items()}, "llm": {}},
)
(outd / "function_results.json").write_text(fd.model_dump_json())
board = Scoreboard(name="FreshBench", description="DecBench-style benchmark on a corpus no tool was tuned on",
                   generated_at=datetime.now(), projects_evaluated=sorted({k[0] for k in groups}),
                   optimization_levels=[f"{c}-{o}" for c in ("gcc", "clang") for o in OPTS],
                   decompilers=TOOLS, metrics=metrics,
                   total_functions=sum(len(v) for v in groups.values()), total_binaries=len(groups))
agg = build_aggregates(fd, board)
ds = build_dataset_page(fd)
(outd / "aggregates.json").write_text(json.dumps(agg, indent=1, default=str))
(outd / "dataset.json").write_text(json.dumps(ds, indent=1, default=str))


def pct(a):
    return f"{100 * a[0] / a[1]:.1f}% ({a[0]:,}/{a[1]:,})" if a and a[1] else "-"


lines = ["# FreshBench — DecBench metrics (computed by decbench.rendering.aggregate)", ""]
for preset in ["full", "gcc", "clang"] + OPTS:
    combo = agg["combos"].get(f"{preset}|raw") or agg["combos"].get(f"{preset}") or next(
        (v for k, v in agg["combos"].items() if k.startswith(preset + "|") and "norm" not in k), None)
    if combo is None:
        continue
    lines += [f"## {preset}", "", "| tool | Union | " + " | ".join(metrics) + " | errors | compiles |",
              "|---|---:|" + "---:|" * len(metrics) + "---:|---:|"]
    order = sorted(TOOLS, key=lambda t: -(combo["overall"][t][0] / max(1, combo["overall"][t][1])))
    for t in order:
        lines.append(f"| {t} {VERSIONS[t]} | {pct(combo['overall'][t])} | "
                     + " | ".join(pct(combo["per_metric"][t].get(m)) for m in metrics)
                     + f" | {pct(combo['errors'][t])} | {pct(combo['compile'][t])} |")
    lines += ["", "distance (mean · median · at-0 / measured)", "",
              "| tool | " + " | ".join(metrics) + " |", "|---|" + "---|" * len(metrics)]
    for t in order:
        cells = []
        for m in metrics:
            d = (combo["distance"].get(t) or {}).get(m)
            cells.append("-" if not d else f"{d.get('mean', 0):.1f} · med {d.get('median', 0):g} · "
                         f"{d.get('at0', 0):,}/{d.get('n', d.get('count', 0)):,}")
        lines.append(f"| {t} | " + " | ".join(cells) + " |")
    lines.append("")
lines += ["## pipeline health", "", f"source CFG lost: {ds['joern']['source']}", "",
          "| tool | Joern failed on output |", "|---|---:|"]
for t in TOOLS:
    lines.append(f"| {t} | {pct(ds['joern']['output'][t])} |")
lines += ["", "## cost (batch wall / function, per binary)", "", "| tool | median s/fn | mean s/fn | functions |",
          "|---|---:|---:|---:|"]
for t, c in agg["cost"].items():
    lines.append(f"| {t} | {c['time']['median_s']:.3f} | {c['time']['mean_s']:.3f} | {c['time']['n_functions']:,} |")
(outd / "summary.md").write_text("\n".join(lines) + "\n")
print("\n".join(lines[:40]))
if "--site" in sys.argv:
    from decbench.rendering.site import build_site
    build_site(board, fd, outd / "site", snapshots=[])
    print("site:", outd / "site")

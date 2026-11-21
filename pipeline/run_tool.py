#!/usr/bin/env python3
"""Decompile and score one prepared build with one decompiler (DecBench code path).

usage: run_tool.py <decbench_checkout> <prepared_build_dir> <tool> <out_dir>

For every binary in <prepared_build_dir>/manifest.json this does exactly what
DecBench's scripts/run_benchmark.py does for a project binary:
  * decompile the STRIPPED copy in a killable, resource-scoped subprocess
    (`_timed_decompile` -> scripts/decompile_one.py, systemd-run scope with
    DecBench's per-binary timeout and memory cap), restricted to the selected
    functions' DWARF addresses;
  * relabel the output to DWARF names (`_relabel_to_dwarf`) and validate native
    provenance against the unstripped original;
  * score with `evaluate_decompilation` (GED + type_match) against the
    rust-joern source CFGs written by prepare.py (`rebuild_cfg`).
Writes <out_dir>/rows.jsonl (one row per selected function), records.jsonl.gz
(decompiled code per function) and run.json (timings, failures, versions).
"""
from __future__ import annotations

import gzip
import hashlib
import importlib.util
import json
import os
import resource
import sys
import time
from pathlib import Path

decbench_root = Path(sys.argv[1]).resolve()
prep = Path(sys.argv[2]).resolve()
tool = sys.argv[3]
out = Path(sys.argv[4]).resolve()
out.mkdir(parents=True, exist_ok=True)
os.environ.setdefault("DECBENCH_DWARF_ABSTRACT_ORIGIN", "1")

spec = importlib.util.spec_from_file_location("run_benchmark", decbench_root / "scripts" / "run_benchmark.py")
rb = importlib.util.module_from_spec(spec)
sys.argv_saved = sys.argv
sys.argv = [str(decbench_root / "scripts" / "run_benchmark.py")]
spec.loader.exec_module(rb)
sys.argv = sys.argv_saved

import decbench.decompilers  # noqa: E402,F401
from decbench.decompilers.provenance import sanitize_native_provenance  # noqa: E402
from decbench.decompilers.registry import DecompilerRegistry  # noqa: E402
from decbench.metrics.registry import MetricRegistry  # noqa: E402
from decbench.pipeline.evaluate import evaluate_decompilation  # noqa: E402
from decbench.publish.cfg_export import rebuild_cfg  # noqa: E402

METRICS = ["ged", "type_match"]
manifest = json.loads((prep / "manifest.json").read_text())
dec_out = out / "decompiled"
dec_out.mkdir(exist_ok=True)
try:
    version = DecompilerRegistry.get(tool).get_version()
except Exception as e:  # noqa: BLE001
    version = f"unknown ({e})"

rows, run = [], {"tool": tool, "version": version, "project": manifest["project"],
                 "compiler": manifest["compiler"], "config": manifest["config"], "binaries": {}}
recs = gzip.open(out / "records.jsonl.gz", "wt")
perfect = {m: MetricRegistry.get(m).perfect_value for m in METRICS}
for binf, info in manifest["binaries"].items():
    orig = prep / "binaries" / binf
    stripped = prep / "stripped" / binf
    if binf.startswith("."):
        # A leading dot (build output dir ".O/") makes Ghidra's project import
        # fail with no output; give every tool the same bytes under a plain name.
        safe = out / "bin" / ("_" + binf.lstrip("."))
        safe.parent.mkdir(exist_ok=True)
        safe.write_bytes(stripped.read_bytes())
        stripped = safe
    amap = {f["address"]: f["name"] for f in info["functions"]}
    names_file = out / f"{binf}.addrs.json"
    names_file.write_text(json.dumps(sorted(amap)))
    ru0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.time()
    res = rb._timed_decompile(stripped, tool, dec_out, str(names_file))
    wall = time.time() - t0
    ru1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    rb._relabel_to_dwarf(res, amap, orig)
    sanitize_native_provenance(res, orig)
    src = json.loads((prep / "source_cfgs" / f"{binf}.json").read_text())["functions"]
    source_cfgs = {n: rebuild_cfg(c) for n, c in src.items()}
    t1 = time.time()
    metrics = evaluate_decompilation(res, source_cfgs, METRICS)
    score_secs = time.time() - t1
    extra = res.decompiler.extra or {}
    run["binaries"][binf] = {
        "functions": len(amap), "decompiled": len(res.functions), "wall_s": round(wall, 2),
        "cpu_s": round((ru1.ru_utime + ru1.ru_stime) - (ru0.ru_utime + ru0.ru_stime), 2),
        "children_maxrss_kb": ru1.ru_maxrss, "score_s": round(score_secs, 2),
        "failure": extra.get("failure", ""), "timed_out": bool(res.decompiler.timeout_occurred),
        "failed_functions": list(res.decompiler.failed_functions or [])[:50],
        "metric_errors": {m: r.errors[:5] for m, r in metrics.items() if r.errors},
    }
    for f in info["functions"]:
        name = f["name"]
        fd = res.functions.get(name)
        code = getattr(fd, "decompiled_code", None) if fd is not None else None
        row = {"project": manifest["project"], "compiler": manifest["compiler"],
               "config": manifest["config"], "binary": binf, "function": name,
               "address": f["address"], "source": f["source"], "tool": tool,
               "decompiled": bool(code),
               "code_sha256": hashlib.sha256(code.encode()).hexdigest() if code else None}
        for m in METRICS:
            r = metrics.get(m)
            v = r.function_results.get(name) if r is not None else None
            row[m] = None if v is None else v.value
            row[f"{m}_perfect"] = None if v is None else (v.value == perfect[m])
            if v is not None and v.metadata:
                row[f"{m}_meta"] = {k: v.metadata[k] for k in list(v.metadata)[:8]
                                    if isinstance(v.metadata[k], (int, float, str, bool, type(None)))}
        rows.append(row)
        if code:
            recs.write(json.dumps({"binary": binf, "function": name, "code": code}) + "\n")
recs.close()
with open(out / "rows.jsonl", "w") as fh:
    for r in rows:
        fh.write(json.dumps(r) + "\n")
run["maxrss_children_kb"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
(out / "run.json").write_text(json.dumps(run, indent=1))
n = len(rows)
print(json.dumps({"tool": tool, "rows": n, "decompiled": sum(r["decompiled"] for r in rows),
                  "ged_perfect": sum(bool(r["ged_perfect"]) for r in rows),
                  "type_perfect": sum(bool(r["type_match_perfect"]) for r in rows)}))

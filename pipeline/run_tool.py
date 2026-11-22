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
import shutil
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

import pickle  # noqa: E402
import subprocess  # noqa: E402

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
perfect = {m: MetricRegistry.get(m).perfect_value for m in METRICS}
pk = out / "pkl"
pk.mkdir(exist_ok=True)
SCORER = Path(__file__).with_name("score_child.py")

# 1) decompile every binary (child CPU/RSS measured here, before any scoring child runs)
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
    (pk / f"{binf}.pkl").write_bytes(pickle.dumps(res))
    extra = res.decompiler.extra or {}
    run["binaries"][binf] = {
        "functions": len(amap), "decompiled": len(res.functions), "wall_s": round(wall, 2),
        "cpu_s": round((ru1.ru_utime + ru1.ru_stime) - (ru0.ru_utime + ru0.ru_stime), 2),
        "children_maxrss_kb": ru1.ru_maxrss,
        "failure": extra.get("failure", ""), "timed_out": bool(res.decompiler.timeout_occurred),
        "failed_functions": list(res.decompiler.failed_functions or [])[:50],
    }
run["maxrss_children_kb"] = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss


def score(binf, metrics, names=None):
    """Run score_child.py; None if the child died (e.g. a native crash)."""
    o = out / "score.json"
    o.unlink(missing_ok=True)
    cmd = [sys.executable, "-I", str(SCORER), str(pk / f"{binf}.pkl"),
           str(prep / "source_cfgs" / f"{binf}.json"), str(o), ",".join(metrics)]
    if names is not None:
        nf = out / "names.json"
        nf.write_text(json.dumps(sorted(names)))
        cmd.append(str(nf))
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode != 0 or not o.exists():
        return None
    return json.loads(o.read_text())


def ged_bisect(binf, names, crashed, chunks):
    """GED for `names`, halving the set while the scorer process dies.

    DecBench parses ALL of a binary's decompiled functions as one C file;
    rust-joern can overflow a fixed-size thread stack on a large file and kill
    the process (seen once: OpenDUNE gcc-O0, Kuna). Halves parse fine, so the
    binary is scored in chunks (recorded); a function that crashes alone gets
    no GED and is listed in scorer_crashed_functions."""
    r = score(binf, ["ged"], names)
    if r is not None:
        chunks.append(len(names))
        return r["ged"]["values"], r["ged"]["meta"]
    if len(names) == 1:
        crashed.extend(names)
        return {}, {}
    names = sorted(names)
    a, am = ged_bisect(binf, names[: len(names) // 2], crashed, chunks)
    b, bm = ged_bisect(binf, names[len(names) // 2:], crashed, chunks)
    return {**a, **b}, {**am, **bm}


# 2) score every binary in a child process
recs = gzip.open(out / "records.jsonl.gz", "wt")
for binf, info in manifest["binaries"].items():
    res = pickle.loads((pk / f"{binf}.pkl").read_bytes())
    t1 = time.time()
    r = score(binf, METRICS)
    crashed: list[str] = []
    chunks: list[int] = []
    if r is None:
        # type_match calibrates a per-binary stack shift, so it stays whole-binary;
        # only GED (rust-joern parse of the decompiled C) is narrowed per function.
        t = score(binf, ["type_match"])
        g, gm = ged_bisect(binf, list(res.functions), crashed, chunks)
        r = {"ged": {"values": g, "meta": gm, "errors": []},
             "type_match": t["type_match"] if t else {"values": {}, "meta": {}, "errors": ["scorer crash"]}}
    run["binaries"][binf]["score_s"] = round(time.time() - t1, 2)
    # byte_match: DecBench's ByteMatchMetric inside the score image, which holds
    # the exact compilers that built this binary (see build/Dockerfile.score).
    bm = {}
    score_image = os.environ.get("SCORE_IMAGE")
    if score_image and shutil.which("docker"):
        t2 = time.time()
        bmdir = out / "bm"
        bmdir.mkdir(exist_ok=True)
        path = ("/opt/clang-as-gcc:" if manifest["compiler"] == "clang" else "") + \
            "/opt/venv/bin:/usr/local/bin:/opt/llvm/bin:/usr/bin:/bin"
        p = subprocess.run(
            ["docker", "run", "--rm", "--network", "none", "-e", f"PATH={path}",
             *(["-e", "FB_DIAG_CC=/usr/local/bin/gcc", "-e", "FB_FINAL_CC=/opt/llvm/bin/clang"]
               if manifest["compiler"] == "clang" else []),
             "-v", f"{pk}:/pkl:ro", "-v", f"{prep / 'binaries'}:/orig:ro", "-v", f"{bmdir}:/bm",
             "-v", f"{Path(__file__).parent}:/p:ro", score_image,
             "python", "/p/score_bytematch.py", f"/pkl/{binf}.pkl", f"/orig/{binf}", f"/bm/{binf}.json"],
            capture_output=True, text=True)
        bmfile = bmdir / f"{binf}.json"
        if p.returncode == 0 and bmfile.exists():
            bmr = json.loads(bmfile.read_text())
            bm = bmr["values"]
            run["binaries"][binf]["byte_match"] = {"compiler": bmr["compiler"], "errors": bmr["errors"],
                                                   "seconds": round(time.time() - t2, 2)}
        else:
            run["binaries"][binf]["byte_match"] = {"error": (p.stderr or "")[-1500:],
                                                   "seconds": round(time.time() - t2, 2)}
    run["binaries"][binf]["scorer_crashed_functions"] = crashed
    run["binaries"][binf]["ged_scored_in_chunks"] = chunks
    run["binaries"][binf]["metric_errors"] = {m: r[m]["errors"] for m in METRICS if r.get(m, {}).get("errors")}
    for f in info["functions"]:
        name = f["name"]
        fd = res.functions.get(name)
        code = getattr(fd, "decompiled_code", None) if fd is not None else None
        row = {"project": manifest["project"], "compiler": manifest["compiler"],
               "config": manifest["config"], "binary": binf, "function": name,
               "address": f["address"], "source": f["source"], "tool": tool,
               "decompiled": bool(code),
               "code_sha256": hashlib.sha256(code.encode()).hexdigest() if code else None}
        if name in crashed:
            row["scorer_crash"] = "ged"
        b = bm.get(name)
        row["byte_match"] = None if b is None or b.get("skipped") else b["value"]
        row["byte_match_perfect"] = None if row["byte_match"] is None else bool(b["perfect"])
        if b is not None:
            row["byte_match_meta"] = {k: v for k, v in b.items() if k not in ("value", "perfect")}
        for m in METRICS:
            v = r.get(m, {}).get("values", {}).get(name)
            row[m] = v
            row[f"{m}_perfect"] = None if v is None else (v == perfect[m])
            meta = r.get(m, {}).get("meta", {}).get(name)
            if meta:
                row[f"{m}_meta"] = meta
        rows.append(row)
        if code:
            recs.write(json.dumps({"binary": binf, "function": name, "code": code}) + "\n")
recs.close()
with open(out / "rows.jsonl", "w") as fh:
    for r in rows:
        fh.write(json.dumps(r) + "\n")
shutil.rmtree(pk, ignore_errors=True)
shutil.rmtree(out / "bm", ignore_errors=True)
(out / "run.json").write_text(json.dumps(run, indent=1))
n = len(rows)
print(json.dumps({"tool": tool, "rows": n, "decompiled": sum(r["decompiled"] for r in rows),
                  "ged_perfect": sum(bool(r["ged_perfect"]) for r in rows),
                  "type_perfect": sum(bool(r["type_match_perfect"]) for r in rows)}))

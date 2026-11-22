#!/usr/bin/env python3
"""DecBench byte_match for one binary's decompiled functions (runs in the score image).

usage: score_bytematch.py <result.pkl> <unstripped_binary> <out.json>
<result.pkl> is the relabelled DecompilationResult run_tool.py pickled. The
compiler is whatever `gcc` resolves to on PATH (GCC 16.2.0, or Clang 23.1.6 via
/opt/clang-as-gcc for clang builds), with DecBench's own flags from the DWARF
producer, fix-up loop and Jaccard comparison (ByteMatchMetric.compute_for_binary).
"""
import json, pickle, shutil, subprocess, sys
from pathlib import Path

import decbench.metrics  # noqa: F401
from decbench.metrics.byte_match import ByteMatchMetric

res = pickle.loads(Path(sys.argv[1]).read_bytes())
res.binary_path = Path(sys.argv[2])
m = ByteMatchMetric()
r = m.compute_for_binary(res)
out = {"compiler": subprocess.run(["gcc", "--version"], capture_output=True, text=True).stdout.split("\n")[0],
       "gcc_path": shutil.which("gcc"), "errors": r.errors[:20], "values": {}}
for name, v in r.function_results.items():
    md = v.metadata or {}
    out["values"][name] = {"value": v.value, "perfect": v.value == m.perfect_value,
                           **{k: md.get(k) for k in ("compilable", "exact_match", "changed_lines",
                                                     "jaccard_similarity", "fixup_iterations", "skipped",
                                                     "reason", "error") if k in md}}
Path(sys.argv[3]).write_text(json.dumps(out))

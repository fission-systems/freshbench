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

import os

import decbench.metrics  # noqa: F401
import decbench.metrics.fixup as fixup
from decbench.metrics.byte_match import ByteMatchMetric

# Clang builds: DecBench's fix-up loop parses GCC's diagnostic wording
# ("implicit declaration of function", "'x' undeclared", ...), which Clang
# phrases differently ("call to undeclared function", "use of undeclared
# identifier"), so with Clang as the compiler the loop could not fix anything.
# Drive the loop with GCC 16.2.0's diagnostics, as DecBench intends, and build
# every object that is compared with Clang 23.1.6, the compiler of the binary.
DIAG_CC, FINAL_CC = os.environ.get("FB_DIAG_CC"), os.environ.get("FB_FINAL_CC")
if DIAG_CC and FINAL_CC:
    _orig = fixup._gcc_compile

    def _diag_then_final(src, obj_path, compiler, flags):
        diag = _orig(src, obj_path, DIAG_CC, flags)
        if diag.returncode != 0:
            return diag
        return _orig(src, obj_path, FINAL_CC, flags)

    fixup._gcc_compile = _diag_then_final

res = pickle.loads(Path(sys.argv[1]).read_bytes())
res.binary_path = Path(sys.argv[2])
m = ByteMatchMetric()
r = m.compute_for_binary(res)
cc = FINAL_CC if (DIAG_CC and FINAL_CC) else "gcc"
out = {"compiler": subprocess.run([cc, "--version"], capture_output=True, text=True).stdout.split("\n")[0],
       "fixup_diagnostics": subprocess.run([DIAG_CC or "gcc", "--version"], capture_output=True, text=True).stdout.split("\n")[0],
       "gcc_path": shutil.which("gcc"), "errors": r.errors[:20], "values": {}}
for name, v in r.function_results.items():
    md = v.metadata or {}
    out["values"][name] = {"value": v.value, "perfect": v.value == m.perfect_value,
                           **{k: md.get(k) for k in ("compilable", "exact_match", "changed_lines",
                                                     "jaccard_similarity", "fixup_iterations", "skipped",
                                                     "reason", "error") if k in md}}
Path(sys.argv[3]).write_text(json.dumps(out))

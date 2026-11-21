#!/usr/bin/env python3
"""Score one pickled DecompilationResult in its own process (see run_tool.py).

usage: score_child.py <result.pkl> <source_cfgs.json> <out.json> <metric,...> [names.json]
A native crash here (rust-joern has segfaulted on one decompiler output) only
loses this process; run_tool.py then narrows the crash down to the function.
"""
import json
import os
import pickle
import sys
from pathlib import Path

os.environ.setdefault("DECBENCH_DWARF_ABSTRACT_ORIGIN", "1")
import decbench.metrics  # noqa: E402,F401
from decbench.pipeline.evaluate import evaluate_decompilation  # noqa: E402
from decbench.publish.cfg_export import rebuild_cfg  # noqa: E402

res = pickle.loads(Path(sys.argv[1]).read_bytes())
src = json.loads(Path(sys.argv[2]).read_text())["functions"]
metrics = sys.argv[4].split(",")
if len(sys.argv) > 5:
    keep = set(json.loads(Path(sys.argv[5]).read_text()))
    res.functions = {k: v for k, v in res.functions.items() if k in keep}
cfgs = {n: rebuild_cfg(c) for n, c in src.items()}
out = evaluate_decompilation(res, cfgs, metrics)
payload = {}
for m, r in out.items():
    payload[m] = {
        "values": {n: v.value for n, v in r.function_results.items()},
        "meta": {n: {k: x for k, x in list((v.metadata or {}).items())[:8]
                     if isinstance(x, (int, float, str, bool, type(None)))}
                 for n, v in r.function_results.items()},
        "errors": list(r.errors[:5]),
    }
Path(sys.argv[3]).write_text(json.dumps(payload))

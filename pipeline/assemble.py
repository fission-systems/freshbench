#!/usr/bin/env python3
"""Assemble the dataset tree (DecBench layout) from qualify + decompile artifacts.

usage: assemble.py <corpus.json> <qual_root> <res_root> <out> [exclude.json]
  corpus.json  {"projects": [{"slug", "repo", "sha", "license", "stars", ...}]}
  qual_root    <slug>/prep.tar (+ license/) from qual-<slug> artifacts
  res_root     <slug>/<build>/<tool>/{rows.jsonl,run.json,records.jsonl.gz,time.txt}

Writes:
  binaries/<build>/<slug>/<bin>            unstripped (-g) originals: grading only
  stripped/<build>/<slug>/<bin>            what every decompiler was given
  sources/<build>/<slug>/<sha16>.<stem>.i  preprocessed TUs owning the selected functions
  pipeline_data/source_cfgs/<build>/<slug>/<bin>.json   rust-joern CFGs (DecBench format)
  projects/<slug>/{source.json,LICENSE*}
  results/<tool>/<build>/<slug>/records.jsonl.gz, run.json
  results/function_results.jsonl           one row per (function, tool)
  configs/<config>/manifest.json           full, gcc, clang, <opt>, <compiler>-<opt>
"""
from __future__ import annotations

import hashlib
import json
import shutil
import sys
import tarfile
from collections import defaultdict
from pathlib import Path

corpus = json.loads(Path(sys.argv[1]).read_text())
qual, res, out = Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4])
# [[slug, binary], ...]: binaries left out of the dataset (e.g. a separate debug-info file with no code)
EXCLUDE = {tuple(x) for x in json.loads(Path(sys.argv[5]).read_text())} if len(sys.argv) > 5 else set()
out.mkdir(parents=True, exist_ok=True)
OPTS = ["O0", "O1", "O2", "O3", "Os", "O2-noinline"]
entries = []
(out / "results").mkdir(exist_ok=True)
fr = open(out / "results" / "function_results.jsonl", "w")
for p in corpus["projects"]:
    slug = p["slug"]
    tmp = out / ".tmp" / slug
    if not tmp.exists():
        tmp.mkdir(parents=True)
        with tarfile.open(qual / slug / "prep.tar") as t:
            t.extractall(tmp, filter="data")
    pd = out / "projects" / slug
    pd.mkdir(parents=True, exist_ok=True)
    for lic in (tmp / "license").glob("*") if (tmp / "license").exists() else []:
        shutil.copy2(lic, pd / lic.name)
    (pd / "source.json").write_text(json.dumps(p, indent=1))
    for comp in ("gcc", "clang"):
        for opt in OPTS:
            build = f"{comp}-{opt}"
            bd = tmp / "prep" / build
            if not (bd / "manifest.json").exists():
                continue
            man = json.loads((bd / "manifest.json").read_text())
            for kind in ("binaries", "stripped"):
                dst = out / kind / build / slug
                dst.mkdir(parents=True, exist_ok=True)
                for f in (bd / kind).iterdir():
                    if (slug, f.name) not in EXCLUDE:
                        shutil.copy2(f, dst / f.name)
            sdst = out / "sources" / build / slug
            sdst.mkdir(parents=True, exist_ok=True)
            for f in (bd / "sources").iterdir():
                shutil.copy2(f, sdst / f.name)
            cdst = out / "pipeline_data" / "source_cfgs" / build / slug
            cdst.mkdir(parents=True, exist_ok=True)
            for f in (bd / "source_cfgs").iterdir():
                if (slug, f.name.removesuffix(".json")) not in EXCLUDE:
                    shutil.copy2(f, cdst / f.name)
            tools = []
            for tdir in sorted((res / slug / build).glob("*")) if (res / slug / build).exists() else []:
                tool = tdir.name
                if not (tdir / "rows.jsonl").exists():
                    continue
                tools.append(tool)
                rdst = out / "results" / tool / build / slug
                rdst.mkdir(parents=True, exist_ok=True)
                for name in ("records.jsonl.gz", "run.json", "time.txt"):
                    if (tdir / name).exists():
                        shutil.copy2(tdir / name, rdst / name)
                for line in (tdir / "rows.jsonl").read_text().splitlines():
                    fr.write(line + "\n")
            for binf, info in man["binaries"].items():
                if (slug, binf) in EXCLUDE:
                    continue
                entries.append({
                    "project": slug, "opt": build, "compiler": comp, "level": opt,
                    "flags": man["flags"], "cc_version": man["cc_version"], "binary": binf,
                    "binary_path": f"binaries/{build}/{slug}/{binf}",
                    "stripped_path": f"stripped/{build}/{slug}/{binf}",
                    "sha256": info["sha256"], "stripped_sha256": info["stripped_sha256"],
                    "strip_tool": info["strip_tool"],
                    "size": (out / "binaries" / build / slug / binf).stat().st_size,
                    "source_cfg_path": f"pipeline_data/source_cfgs/{build}/{slug}/{binf}.json",
                    "results": tools,
                    "functions": [f["name"] for f in info["functions"]],
                    "function_info": info["functions"],
                })
    # one project's prep tree at a time: the extracted trees of 100 projects do not fit next to the output
    shutil.rmtree(tmp, ignore_errors=True)
fr.close()
shutil.rmtree(out / ".tmp", ignore_errors=True)

configs = {"full": lambda e: True, "gcc": lambda e: e["compiler"] == "gcc",
           "clang": lambda e: e["compiler"] == "clang"}
for o in OPTS:
    configs[o] = (lambda o: lambda e: e["level"] == o)(o)
    for c in ("gcc", "clang"):
        configs[f"{c}-{o}"] = (lambda c, o: lambda e: e["compiler"] == c and e["level"] == o)(c, o)
for name, pred in configs.items():
    sel = [e for e in entries if pred(e)]
    d = out / "configs" / name
    d.mkdir(parents=True, exist_ok=True)
    (d / "manifest.json").write_text(json.dumps({
        "config": name, "dataset": "FreshBench", "metrics": ["ged", "type_match", "byte_match"],
        "function_count": sum(len(e["functions"]) for e in sel), "binary_count": len(sel),
        "scores": "results/function_results.jsonl",
        "projects": {p["slug"]: {"repo": p["repo"], "sha": p["sha"], "license": p.get("license")}
                     for p in corpus["projects"]},
        "binaries": sel}, indent=1))
print(json.dumps({"binaries": len(entries), "functions": sum(len(e["functions"]) for e in entries)}))

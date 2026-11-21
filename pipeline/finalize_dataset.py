#!/usr/bin/env python3
"""Post-process an assembled dataset tree (run with the decbench venv, -I).

* sources/: keep DecBench's form, the header-stripped TU text
  (decbench.utils.cfg.strip_system_headers), as <sha16>.<stem>.c, one file per
  distinct text per (build, project); the sha is the manifest's tu_sha256.
* hidden names: a binary whose name starts with "." (".O/usr/bin/x") is
  renamed "_..." everywhere (files, manifests, scores), the same name the
  decompilers were given.
usage: finalize_dataset.py <dataset_dir>
"""
import gzip, json, os, sys
from pathlib import Path
from decbench.utils.cfg import strip_system_headers

root = Path(sys.argv[1])
n_in = n_out = 0
for build_dir in (root / "sources").iterdir():
    for proj in build_dir.iterdir():
        for f in list(proj.iterdir()):
            if f.suffix != ".i":
                continue
            n_in += f.stat().st_size
            text = strip_system_headers(f.read_text(errors="replace"))
            out = f.with_suffix(".c")
            out.write_text(text)
            n_out += out.stat().st_size
            f.unlink()
print(f"sources: {n_in/1e6:.0f} MB -> {n_out/1e6:.0f} MB")


def fix(name: str) -> str:
    return "_" + name.lstrip(".") if name.startswith(".") else name


for kind in ("binaries", "stripped", "pipeline_data/source_cfgs"):
    for p in list((root / kind).rglob(".*")):
        p.rename(p.with_name(fix(p.name)))
for man in (root / "configs").glob("*/manifest.json"):
    m = json.loads(man.read_text())
    for b in m["binaries"]:
        if b["binary"].startswith("."):
            old = b["binary"]
            b["binary"] = fix(old)
            for k in ("binary_path", "stripped_path", "source_cfg_path"):
                b[k] = b[k].replace("/" + old, "/" + b["binary"])
        for f in b.get("function_info", []):
            hit = sorted((root / "sources" / b["opt"] / b["project"]).glob(f"{f['tu_sha256'][:16]}.*.c"))
            assert len(hit) == 1, (b["binary"], f["name"], hit)
            f["source_path"] = str(hit[0].relative_to(root))
    man.write_text(json.dumps(m, indent=1))
fr = root / "results" / "function_results.jsonl"
lines = []
for l in fr.read_text().splitlines():
    r = json.loads(l)
    r["binary"] = fix(r["binary"])
    lines.append(json.dumps(r))
fr.write_text("\n".join(lines) + "\n")
for rec in (root / "results").rglob("records.jsonl.gz"):
    rows = [json.loads(l) for l in gzip.open(rec, "rt")]
    if any(r["binary"].startswith(".") for r in rows):
        with gzip.open(rec, "wt") as g:
            for r in rows:
                r["binary"] = fix(r["binary"]); g.write(json.dumps(r) + "\n")
for cfg in (root / "pipeline_data/source_cfgs").rglob("*.json"):
    d = json.loads(cfg.read_text())
    if d.get("binary", "").startswith("."):
        d["binary"] = fix(d["binary"]); cfg.write_text(json.dumps(d, indent=2))
print("hidden left:", [str(p) for p in root.rglob(".*")][:5])

#!/usr/bin/env python3
"""Fingerprint every DecBench source function so FreshBench can exclude overlap.

A function's fingerprint is sha256 over its sorted CFG node labels with the
leading "<line>.<col>:" position stripped (statement text only), so the same
code at different line numbers still matches. A second, weaker key is
(name, node count, edge count). FreshBench drops a selected function if either
key matches any DecBench function (dataset 815eb96f, published + rebuilt CFGs).
usage: decbench_fingerprints.py <cfg_root>... > decbench_fingerprints.json
"""
import glob, hashlib, json, re, sys

POS = re.compile(r"^-?\d+(\.\d+)?:\n")


def fp(func):
    labels = sorted(POS.sub("", l) for l in func.get("labels", {}).values())
    return hashlib.sha256("\x00".join(labels).encode()).hexdigest()


strong, weak = set(), set()
files = 0
for root in sys.argv[1:]:
    for p in glob.glob(f"{root}/**/*.json", recursive=True):
        try:
            d = json.load(open(p))
        except Exception:
            continue
        for name, f in (d.get("functions") or {}).items():
            strong.add(fp(f))
            weak.add(f"{name}|{len(f.get('nodes', []))}|{len(f.get('edges', []))}")
        files += 1
json.dump({"files": files, "strong": sorted(strong), "weak": sorted(weak)}, sys.stdout)
print(f"{files} files, {len(strong)} strong, {len(weak)} weak", file=sys.stderr)

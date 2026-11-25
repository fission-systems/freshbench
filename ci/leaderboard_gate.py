#!/usr/bin/env python3
"""Validate the M1 target population and result completeness before publication."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path

REVISION = "15b6f2c52c379cf9361f22edaa75886602e5e1ff"
QUALIFY_RUN = "45460924446"
EXPECTED_TARGETS = 285875
KEY_FIELDS = ("project", "arch", "compiler", "config", "binary", "function")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load_targets(root, manifest, verify_sources=False):
    expected = set()
    with gzip.open(manifest, "rt") as source:
        entries = [json.loads(line) for line in source if line.strip()]
    truth = {entry["path"]: entry for entry in entries
             if entry["path"].startswith("truth/") and entry["path"].endswith("/targets.json")}
    require(len(truth) == 960, f"expected 960 M1 truth files, got {len(truth)}")
    actual = {p.relative_to(root).as_posix() for p in (root / "truth").rglob("targets.json")}
    require(actual == set(truth), "downloaded truth file set differs from pinned manifest")
    for rel, entry in sorted(truth.items()):
        data = (root / rel).read_bytes()
        require(len(data) == entry["size"] and hashlib.sha256(data).hexdigest() == entry["sha256"],
                f"truth digest mismatch: {rel}")
        unit = json.loads(data)
        for binary, record in unit["binaries"].items():
            for function in record["functions"]:
                key = (unit["project"], unit["arch"], unit["compiler"], unit["config"],
                       binary, function["name"])
                require(key not in expected, f"duplicate target: {key}")
                expected.add(key)
    if verify_sources:
        sources = {entry["path"]: entry for entry in entries
                   if entry["path"].startswith("sources/")}
        require(bool(sources), "manifest has no source files")
        for rel, entry in sorted(sources.items()):
            path = root / rel
            require(path.is_file() and path.stat().st_size == entry["size"],
                    f"source missing or wrong size: {rel}")
            with path.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            require(digest == entry["sha256"], f"source digest mismatch: {rel}")
    require(len(expected) == EXPECTED_TARGETS, f"unexpected M1 row count: {len(expected)}")
    return expected


def validate_rows(rows, expected, registry):
    seen = {}
    scorers = set()
    for file in sorted(rows.glob("*.jsonl")):
        if file.name.endswith(".recompile.jsonl"):
            continue
        with file.open() as source:
            for line in source:
                if not line.strip():
                    continue
                row = json.loads(line)
                key = tuple(row[field] for field in KEY_FIELDS)
                tool = row["tool"]
                require(tool in registry, f"unknown tool: {tool}")
                require(not registry[tool].get("llm", False), f"LLM tool in M1 ranking: {tool}")
                require(key in expected, f"unexpected target: {key}")
                require(bool(row["scorer"]), "empty scorer ID")
                scorers.add(row["scorer"])
                seen.setdefault(tool, set()).add(key)
    require(len(scorers) == 1, f"expected one scorer ID, got {scorers}")
    require(len(seen) >= 2, "at least two tools with results are required for a leaderboard")
    incomplete = []
    for tool, keys in sorted(seen.items()):
        missing = len(expected - keys)
        print(f"{tool}: {len(keys)}/{len(expected)} rows ({missing} missing)")
        if missing:
            incomplete.append(tool)
    require(not incomplete, f"incomplete result sets: {', '.join(incomplete)}")
    return set(seen)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--verify-sources", action="store_true")
    parser.add_argument("--rows", type=Path, required=True)
    parser.add_argument("--meta", type=Path, required=True)
    parser.add_argument("--registry", type=Path, required=True)
    args = parser.parse_args()
    meta = json.loads(args.meta.read_text())
    require(meta["pins"]["corpus build (qualify-arch run)"] == QUALIFY_RUN,
            "result provenance does not match M1 qualification run")
    registry = json.loads(args.registry.read_text())
    validate_rows(args.rows, load_targets(args.corpus, args.manifest, args.verify_sources), registry)
    print(f"M1 publication gate passed: {REVISION}")


if __name__ == "__main__":
    main()

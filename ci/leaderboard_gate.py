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


BINARY_FIELDS = KEY_FIELDS[:5]
VARIANTS = ("as-built", "no-unwind")


def load_targets(root, manifest):
    return load_population(root, manifest)[0]


def load_population(root, manifest):
    """(every M1 target, every M1 binary) from the pinned truth files."""
    expected = set()
    binaries = set()
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
            binaries.add((unit["project"], unit["arch"], unit["compiler"], unit["config"], binary))
            for function in record["functions"]:
                key = (unit["project"], unit["arch"], unit["compiler"], unit["config"],
                       binary, function["name"])
                require(key not in expected, f"duplicate target: {key}")
                expected.add(key)
    require(len(expected) == EXPECTED_TARGETS, f"unexpected M1 row count: {len(expected)}")
    return expected, binaries


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
                # no instruction index for the target: an infrastructure or
                # truth failure, which must not be published as the tool's
                require("no_instruction_index" not in row.get("tags", []),
                        f"row without an instruction index: {tool} {key}")
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


def validate_boundaries(dirs, binaries, registry):
    """Every tool with boundary rows has one for every M1 binary in both
    variants. A binary without a row is not counted as found or missed by
    the summary, so a tool whose job was cancelled would be scored on fewer
    binaries than the others; an explicit failure (`error`) is a row."""
    seen = {}
    for d in dirs:
        for file in sorted(Path(d).rglob("*.jsonl")):
            if file.parent.name != "boundaries":
                continue
            with file.open() as source:
                for n, line in enumerate(source, 1):
                    if not line.strip():
                        continue
                    row = json.loads(line)
                    tool, variant = row["tool"], row["variant"]
                    require(tool in registry, f"unknown tool in boundary rows: {tool}")
                    require(not registry[tool].get("llm", False), f"LLM tool in M1 boundaries: {tool}")
                    require(variant in VARIANTS, f"{file}:{n}: unknown variant {variant}")
                    key = tuple(row[field] for field in BINARY_FIELDS)
                    require(key in binaries, f"{file}:{n}: unexpected binary {key}")
                    mine = seen.setdefault((tool, variant), {})
                    require(key not in mine or mine[key] == row,
                            f"boundary rows disagree: {tool} {variant} {key}")
                    mine[key] = row
    incomplete = []
    for tool in sorted({t for t, _ in seen}):
        for variant in VARIANTS:
            have = seen.get((tool, variant), {})
            missing = len(binaries - set(have))
            print(f"{tool} {variant}: {len(have)}/{len(binaries)} binaries ({missing} missing)")
            if missing:
                incomplete.append(f"{tool} {variant}")
    require(not incomplete, f"incomplete boundary results: {', '.join(incomplete)}")
    return {t for t, _ in seen}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--rows", type=Path, required=True)
    parser.add_argument("--meta", type=Path, required=True)
    parser.add_argument("--registry", type=Path, required=True)
    parser.add_argument("--boundaries", type=Path, nargs="*", default=[],
                        help="directories holding the boundary track's boundaries/*.jsonl")
    args = parser.parse_args()
    meta = json.loads(args.meta.read_text())
    require(meta["pins"]["corpus build (qualify-arch run)"] == QUALIFY_RUN,
            "result provenance does not match M1 qualification run")
    registry = json.loads(args.registry.read_text())
    targets, binaries = load_population(args.corpus, args.manifest)
    validate_rows(args.rows, targets, registry)
    validate_boundaries(args.boundaries, binaries, registry)
    print(f"M1 publication gate passed: {REVISION}")


if __name__ == "__main__":
    main()

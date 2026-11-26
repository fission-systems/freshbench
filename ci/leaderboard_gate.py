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


def sha256_text(text):
    return hashlib.sha256(text.encode()).hexdigest()


def truth_facts(unit, record):
    """What a reuse key of this binary must say about it: the stripped file and
    the functions requested (`crates/freshbench/src/reuse.rs`: the address with
    the Thumb bit, then the name, in address order)."""
    funcs = {f["address"] | int(bool(f.get("thumb"))): f["name"] for f in record["functions"]}
    return {"stripped_sha256": record.get("stripped_sha256"),
            "requests_sha256": sha256_text("".join(f"{a:#x} {funcs[a]}\n" for a in sorted(funcs)))}


def load_population(root, manifest):
    """(every M1 target, every M1 binary, reuse facts of each binary by
    (build, project, binary)) from the pinned truth files."""
    expected = set()
    binaries = set()
    facts = {}
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
            facts[(rel.split("/")[1], unit["project"], binary)] = truth_facts(unit, record)
            for function in record["functions"]:
                key = (unit["project"], unit["arch"], unit["compiler"], unit["config"],
                       binary, function["name"])
                require(key not in expected, f"duplicate target: {key}")
                expected.add(key)
    require(len(expected) == EXPECTED_TARGETS, f"unexpected M1 row count: {len(expected)}")
    return expected, binaries, facts


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


def reuse_key(parts):
    """`reuse::key`: SHA-256 of the compact JSON with sorted names."""
    return sha256_text(json.dumps(parts, sort_keys=True, separators=(",", ":"), ensure_ascii=False))


def validate_reuse(dirs, facts, registry):
    """Check what every job's `provenance.json` says about reused outputs: each
    key is the hash of its parts, the parts describe this job's tool, version
    and track and this corpus's binary and requests, an output taken from an
    earlier run was recorded there under exactly the same key, fission is
    never reused, and one tool has one version, adapter and harness in the
    whole table. Returns (entries, reused entries)."""
    seen = {}
    total = reused = 0
    for d in dirs:
        for file in sorted(Path(d).rglob("provenance.json")):
            job = json.loads(file.read_text())
            for e in job.get("reuse", []):
                total += 1
                parts = e["parts"]
                tool = job["tool"]
                where = f"{file}: {tool} {parts.get('project')} {parts.get('binary')}"
                require(e["key"] == reuse_key(parts), f"{where}: key is not the hash of its parts")
                require(parts["tool"] == tool and tool in registry, f"{where}: key of another tool")
                require(parts["tool_version"] == job["tool_version"],
                        f"{where}: key of another tool version")
                require(parts["track"] == job["track"], f"{where}: key of another track")
                require(parts["project"] == job["project"], f"{where}: key of another project")
                truth = facts.get((parts["build"], parts["project"], parts["binary"]))
                require(truth is not None, f"{where}: not an M1 binary")
                require(parts["stripped_sha256"] == truth["stripped_sha256"],
                        f"{where}: key of another binary file")
                if parts["track"] == "decompile":
                    require(parts["requests_sha256"] == truth["requests_sha256"],
                            f"{where}: key of other requested functions")
                elif parts["variant"] == "as-built":
                    require(parts["input_sha256"] == truth["stripped_sha256"],
                            f"{where}: key of another input")
                if e.get("reused_from") is not None:
                    reused += 1
                    require(tool != "fission", f"{where}: a fission output was reused")
                    require(str(e["reused_from"]).isdigit(), f"{where}: reused from no run")
                    require(e.get("source_key") == e["key"],
                            f"{where}: reused output's key does not match")
                    require(e.get("reusable") is True, f"{where}: reused an output marked unusable")
                one = (parts["tool_version"], parts["adapter_sha256"], parts["harness_sha256"])
                require(seen.setdefault(tool, one) == one,
                        f"{tool}: outputs of two versions, adapters or harnesses in one table")
    print(f"reuse: {reused} of {total} recorded outputs taken from earlier runs")
    return total, reused


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--rows", type=Path, required=True)
    parser.add_argument("--meta", type=Path, required=True)
    parser.add_argument("--registry", type=Path, required=True)
    parser.add_argument("--boundaries", type=Path, nargs="*", default=[],
                        help="directories holding the boundary track's boundaries/*.jsonl")
    parser.add_argument("--provenance", type=Path, nargs="*", default=[],
                        help="directories holding the jobs' provenance.json")
    args = parser.parse_args()
    meta = json.loads(args.meta.read_text())
    require(meta["pins"]["corpus build (qualify-arch run)"] == QUALIFY_RUN,
            "result provenance does not match M1 qualification run")
    registry = json.loads(args.registry.read_text())
    targets, binaries, facts = load_population(args.corpus, args.manifest)
    validate_rows(args.rows, targets, registry)
    validate_boundaries(args.boundaries, binaries, registry)
    validate_reuse(args.provenance, facts, registry)
    print(f"M1 publication gate passed: {REVISION}")


if __name__ == "__main__":
    main()

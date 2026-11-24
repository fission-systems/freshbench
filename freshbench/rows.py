"""Reading FreshBench result rows and the texts they refer to.

Two layouts are read:

* a dataset tree (DecBench-site layout, as published on Hugging Face):
    results/function_results.jsonl            one row per (tool, function)
    results/<tool>/<build>/<project>/records.jsonl.gz   decompiled code
    configs/<opt>/manifest.json               binaries, function_info.source_path
    sources/<build>/<project>/<tu>.c          preprocessed translation units
* a run tree (what one CI job writes):
    <project>/<build>/<tool>/rows.jsonl and records.jsonl.gz

A build is "<compiler>-<config>" for x86-64 (the v1 names) and
"<arch>-<compiler>-<config>" for every other architecture.
"""
from __future__ import annotations

import gzip
import json
import os
from collections.abc import Iterator
from dataclasses import dataclass

ARCHES = ("x86_64", "i686", "aarch64", "armv7")


@dataclass(frozen=True, order=True)
class RowKey:
    project: str
    arch: str
    compiler: str
    config: str
    binary: str
    function: str

    @property
    def build(self) -> str:
        b = f"{self.compiler}-{self.config}"
        return b if self.arch == "x86_64" else f"{self.arch}-{b}"

    def as_id(self) -> str:
        return f"{self.project}/{self.build}/{self.binary}/{self.function}"


def key_of(row: dict) -> RowKey:
    return RowKey(row["project"], row.get("arch") or "x86_64", row["compiler"], row["config"],
                  row["binary"], row["function"])


def iter_jsonl(path: str) -> Iterator[dict]:
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                yield json.loads(line)


def iter_rows(root: str) -> Iterator[dict]:
    """Result rows under a dataset tree or a run tree, in a deterministic order."""
    flat = os.path.join(root, "results", "function_results.jsonl")
    if os.path.exists(flat):
        yield from iter_jsonl(flat)
        return
    for dirpath, dirnames, filenames in sorted_walk(root):
        if "rows.jsonl" in filenames:
            yield from iter_jsonl(os.path.join(dirpath, "rows.jsonl"))


def sorted_walk(root: str):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        yield dirpath, dirnames, sorted(filenames)


def load_code(root: str, tool: str, wanted: set[RowKey] | None = None) -> dict[RowKey, str]:
    """Decompiled code of one tool, keyed by row. Only rows in `wanted` when given."""
    out: dict[RowKey, str] = {}
    base = os.path.join(root, "results", tool)
    if os.path.isdir(base):
        # dataset tree: results/<tool>/<build>/<project>/records.jsonl.gz
        for build in sorted(os.listdir(base)):
            arch, compiler, config = split_build(build)
            bdir = os.path.join(base, build)
            for project in sorted(os.listdir(bdir)):
                rec = os.path.join(bdir, project, "records.jsonl.gz")
                if os.path.exists(rec):
                    _read_records(rec, project, arch, compiler, config, wanted, out)
        return out
    # run tree: <project>/<build>/<tool>/records.jsonl.gz
    for dirpath, _, filenames in sorted_walk(root):
        if os.path.basename(dirpath) == tool and "records.jsonl.gz" in filenames:
            build_dir = os.path.dirname(dirpath)
            project = os.path.basename(os.path.dirname(build_dir))
            arch, compiler, config = split_build(os.path.basename(build_dir))
            _read_records(os.path.join(dirpath, "records.jsonl.gz"), project, arch, compiler,
                          config, wanted, out)
    return out


def _read_records(path, project, arch, compiler, config, wanted, out):
    for r in iter_jsonl(path):
        k = RowKey(project, arch, compiler, config, r["binary"], r["function"])
        if wanted is None or k in wanted:
            out[k] = r.get("code") or ""


def split_build(build: str) -> tuple[str, str, str]:
    """"gcc-O2-noinline" -> ("x86_64", "gcc", "O2-noinline"); "aarch64-clang-Os" -> ("aarch64", "clang", "Os")."""
    parts = build.split("-")
    arch = "x86_64"
    if parts[0] in ARCHES:
        arch, parts = parts[0], parts[1:]
    if len(parts) < 2 or parts[0] not in ("gcc", "clang", "msvc", "mingw"):
        raise ValueError(f"not a build name: {build}")
    return arch, parts[0], "-".join(parts[1:])


def source_paths(root: str) -> dict[RowKey, str]:
    """Row -> preprocessed TU path (relative to root), from the dataset's config manifests."""
    out: dict[RowKey, str] = {}
    cdir = os.path.join(root, "configs")
    if not os.path.isdir(cdir):
        return out
    for cfg in sorted(os.listdir(cdir)):
        mpath = os.path.join(cdir, cfg, "manifest.json")
        if not os.path.exists(mpath):
            continue
        with open(mpath, encoding="utf-8") as f:
            m = json.load(f)
        for b in m.get("binaries", []):
            arch = b.get("arch") or "x86_64"
            for fi in b.get("function_info", []):
                if fi.get("source_path"):
                    k = RowKey(b["project"], arch, b["compiler"], b["level"], b["binary"], fi["name"])
                    out[k] = fi["source_path"]
    return out

"""Corpus manifests: every file of a milestone with its hash and provenance.

A manifest is JSON lines, sorted by path, one object per file:

  {"path": "binaries/aarch64-gcc-O2/<project>/<binary>", "sha256": ..., "size": ...,
   "kind": "binary", "project": ..., "arch": ..., "compiler": ..., "config": ...,
   "compiler_version": "gcc (GCC) 16.2.0 target=aarch64-linux-gnu", "flags": "-O2",
   "source_repo": ..., "source_commit": ..., "license": "MIT"}

`kind` is one of binary, stripped, source, source_cfg, license, other. Rows
that are not binaries carry whatever provenance applies. The manifest is
committed to git; the files themselves live on Hugging Face at a pinned
revision. `verify` re-hashes a downloaded tree against it.
"""
from __future__ import annotations

import hashlib
import json
import os
from collections import Counter
from collections.abc import Iterable

from .rows import split_build

KINDS = {"binaries": "binary", "stripped": "stripped", "sources": "source",
         "pipeline_data": "source_cfg", "projects": "license"}


def sha256_file(path: str, bufsize: int = 1 << 20) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(bufsize):
            h.update(chunk)
    return h.hexdigest()


def classify_path(rel: str) -> dict:
    """Kind and (project, build) of a dataset-relative path."""
    parts = rel.split("/")
    kind = KINDS.get(parts[0], "other")
    info: dict = {"kind": kind}
    build_idx = {"binary": 1, "stripped": 1, "source": 1, "source_cfg": 2}.get(kind)
    if build_idx is not None and len(parts) > build_idx + 1:
        try:
            arch, compiler, config = split_build(parts[build_idx])
        except ValueError:
            return info
        info.update(arch=arch, compiler=compiler, config=config, project=parts[build_idx + 1])
    elif kind == "license" and len(parts) > 1:
        info["project"] = parts[1]
    return info


def build_manifest(root: str, provenance: dict[tuple[str, str], dict] | None = None,
                   projects: dict[str, dict] | None = None,
                   skip: Iterable[str] = ("README.md", ".gitattributes")) -> list[dict]:
    """Manifest entries for every file under `root`.

    provenance: (project, build) -> {"compiler_version", "flags", ...} merged
    into binary/stripped entries. projects: project -> {"repo", "sha",
    "license"} merged into every entry of that project.
    """
    skip = set(skip)
    entries = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
        for fn in sorted(filenames):
            full = os.path.join(dirpath, fn)
            rel = os.path.relpath(full, root).replace(os.sep, "/")
            if rel in skip or os.path.islink(full):
                continue
            e = {"path": rel, "sha256": sha256_file(full), "size": os.path.getsize(full)}
            e.update(classify_path(rel))
            p = e.get("project")
            if provenance and e["kind"] in ("binary", "stripped") and p:
                b = rel.split("/")[1]
                e.update(provenance.get((p, b), {}))
            if projects and p in projects:
                pr = projects[p]
                e.update(source_repo=pr.get("repo"), source_commit=pr.get("sha"),
                         license=pr.get("license"))
            entries.append(e)
    entries.sort(key=lambda e: e["path"])
    return entries


def write_manifest(entries: list[dict], path: str) -> None:
    with open(path, "w", encoding="utf-8") as f:
        for e in entries:
            f.write(json.dumps(e, sort_keys=True) + "\n")


def read_manifest(path: str) -> list[dict]:
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def verify(root: str, entries: list[dict], check_extra: bool = True) -> list[str]:
    """Problems found: missing files, size/hash mismatches, files not in the manifest."""
    problems = []
    listed = set()
    for e in entries:
        rel = e["path"]
        listed.add(rel)
        full = os.path.join(root, rel)
        if not os.path.exists(full):
            problems.append(f"missing {rel}")
            continue
        if os.path.getsize(full) != e["size"]:
            problems.append(f"size {rel}: {os.path.getsize(full)} != {e['size']}")
            continue
        if sha256_file(full) != e["sha256"]:
            problems.append(f"sha256 {rel}")
    if check_extra:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if not d.startswith(".")]
            for fn in filenames:
                rel = os.path.relpath(os.path.join(dirpath, fn), root).replace(os.sep, "/")
                if rel not in listed and rel not in ("README.md", ".gitattributes"):
                    problems.append(f"unlisted {rel}")
    return sorted(problems)


def summarize(entries: list[dict]) -> dict:
    """Totals per kind and per (arch, compiler, config) for binaries."""
    by_kind = Counter()
    bytes_by_kind = Counter()
    builds = Counter()
    licenses = {}
    versions = {}
    for e in entries:
        by_kind[e["kind"]] += 1
        bytes_by_kind[e["kind"]] += e["size"]
        if e["kind"] == "binary":
            builds[f"{e.get('arch')}/{e.get('compiler')}/{e.get('config')}"] += 1
            if e.get("compiler_version"):
                versions[f"{e.get('arch')}/{e.get('compiler')}"] = e["compiler_version"]
        if e.get("project") and e.get("license"):
            licenses[e["project"]] = e["license"]
    return {
        "files": dict(sorted(by_kind.items())),
        "bytes": dict(sorted(bytes_by_kind.items())),
        "total_bytes": sum(bytes_by_kind.values()),
        "binaries_per_build": dict(sorted(builds.items())),
        "compiler_versions": dict(sorted(versions.items())),
        "licenses": dict(sorted(Counter(licenses.values()).items())),
        "projects": len({e["project"] for e in entries if e.get("project")}),
    }

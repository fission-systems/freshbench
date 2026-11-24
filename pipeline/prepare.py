#!/usr/bin/env python3
"""Turn one project's 12 builds into benchmark rows (run with -I, decbench venv).

usage: prepare.py <project> <builds_dir> <out_dir> [--submodules a,b] [--select N]

<builds_dir>/<compiler>-<config>/ holds build.json, tu_log.jsonl, i/, bin/ as
written by build_project.py.

A function is ELIGIBLE in a build when all of these hold:
  * DW_TAG_subprogram with DW_AT_low_pc in an x86-64 binary of that build
    (name and decl_file read through DW_AT_abstract_origin, i.e. DecBench's
    DECBENCH_DWARF_ABSTRACT_ORIGIN=1 policy, so out-of-line bodies of
    partly-inlined functions count);
  * its compile unit is C (DW_AT_language C89..C23) and its DW_AT_producer
    shows the pinned compiler and optimization flags (the wrapper really ran);
  * it is declared in the compile unit's own primary source file (not a header),
    which lies inside the project tree, outside any submodule and outside
    third-party / test / example / benchmark / fuzz directories (EXCLUDED_DIRS);
  * exactly one preprocessed text exists for that source file in this build
    (a file compiled twice with different macros is ambiguous and dropped);
  * rust-joern (DecBench's extract_cfgs_from_source) yields a CFG for the name;
  * the name is not in DecBench's SKIP_NAMES and occurs at one address only in
    its binary;
  * DW_AT_low_pc lies inside an executable section that holds bytes in the file
    (a function the linker discarded with --gc-sections keeps its DWARF with
    low_pc 0, and a separate debug-info file, `objcopy --only-keep-debug`, keeps
    `.text` as SHT_NOBITS: no code to decompile).
Selection: eligible functions ordered by
  sha256("freshbench-v1/<project>/<source path>/<name>"), first N (default 300)
  per build. A project qualifies when both O0 builds have >= N eligible.
Each function is scored in one binary: the first, in sorted path order, that
defines it.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

os.environ.setdefault("DECBENCH_DWARF_ABSTRACT_ORIGIN", "1")

from decbench.decompilers.raw.common import SKIP_NAMES  # noqa: E402
from decbench.publish.cfg_export import _stripped_sha, _write_cfg_json, relabel_cfg  # noqa: E402
from decbench.utils import binfmt  # noqa: E402
from decbench.utils.cfg import extract_cfgs_from_source  # noqa: E402

CONFIGS = ["O0", "O1", "O2", "O3", "Os", "O2-noinline"]
COMPILERS = ["gcc", "clang"]
C_LANGS = {0x1, 0x2, 0xC, 0x1D, 0x2C, 0x2D}  # C89, C, C99, C11, C17, C23 (DW_LANG_*)
EXCLUDED_DIRS = re.compile(
    r"(^|/)(third[_-]?party|3rd[_-]?party|thirdparty|vendor|vendored|deps|external|extern|"
    r"contrib|submodules?|test|tests|testing|testsuite|unittests?|t|example|examples|demo|"
    r"demos|sample|samples|bench|benchmarks?|fuzz|fuzzing|fuzzers?)(/|$)",
    re.I,
)


# Well-known single-file / amalgamated libraries that projects copy in; a
# function from a source file with one of these basenames is not project code.
VENDORED_BASENAMES = {
    "sqlite3.c", "shell.c", "cjson.c", "cjson_utils.c", "miniz.c", "lodepng.c", "ini.c",
    "linenoise.c", "jsmn.c", "parson.c", "sds.c", "ae.c", "anet.c", "zmalloc.c", "mongoose.c",
    "civetweb.c", "tinyexpr.c", "yxml.c", "argparse.c", "getopt.c", "getopt_long.c", "md5.c",
    "sha1.c", "sha256.c", "sha512.c", "crc32.c", "base64.c", "utf8proc.c", "xxhash.c",
    "lz4.c", "lz4hc.c", "zstd.c", "adler32.c", "inflate.c", "deflate.c", "trees.c",
    "inftrees.c", "inffast.c", "zutil.c", "lua.c", "lapi.c", "lvm.c", "stb_image.c",
    "stb_truetype.c", "nuklear.c", "cimgui.c", "microui.c", "log.c", "uthash.c", "list.c",
    "klib.c", "khash.c", "tinycthread.c", "pcg_basic.c", "mpc.c", "tweetnacl.c", "monocypher.c",
    "aes.c", "rijndael.c", "blake2b.c", "blake2s.c", "siphash.c", "murmurhash3.c", "fnmatch.c",
    "regex.c", "strlcpy.c", "strlcat.c", "reallocarray.c", "asprintf.c", "queue.c",
}


def expected_producer(compiler: str, config: str) -> list[str]:
    opt = {"O2-noinline": "-O2"}.get(config, "-" + config)
    need = [opt]
    if config == "O2-noinline":
        need.append("-fno-inline")
    return need


def producer_ok(prod: str, compiler: str, config: str) -> bool:
    if compiler == "gcc" and not prod.startswith("GNU C"):
        return False
    if compiler == "clang" and "clang version" not in prod:
        return False
    toks = prod.split()
    opts = [t for t in toks if re.fullmatch(r"-O[0-3sz]?", t)]
    need = expected_producer(compiler, config)
    if not opts or opts[-1] != need[0]:
        return False
    return all(n in toks for n in need[1:])


OVERLAP = None
_POS = re.compile(r"^-?\d+(\.\d+)?:\n")


def overlaps(name, serial) -> bool:
    nodes, edges, labels = serial[0], serial[1], serial[2]
    if len(nodes) >= 3 and f"{name}|{len(nodes)}|{len(edges)}" in OVERLAP["weak"]:
        return True
    if len(nodes) >= 4:
        fp = hashlib.sha256("\x00".join(sorted(_POS.sub("", l) for l in labels.values())).encode())
        if fp.hexdigest() in OVERLAP["strong"]:
            return True
    return False


def full_file_table(dw, cu, cache: dict) -> list:
    """The CU's line-table file names as normalised absolute paths, indexed by
    DW_AT_decl_file (DWARF 5: 0-based; DWARF 2-4: 1-based, slot 0 = None).
    binfmt.cu_file_table returns basenames only, which cannot tell sub/x.c
    from x.c, so the directory is resolved here."""
    off = cu.cu_offset
    if off in cache:
        return cache[off]
    top = cu.get_top_DIE()
    comp_dir = binfmt.die_str_attr(top, "DW_AT_comp_dir") or ""
    lp = dw.line_program_for_CU(cu)
    out: list = []
    if lp is not None:
        hdr = lp.header
        v5 = hdr["version"] >= 5
        dec = lambda x: x.decode(errors="replace") if isinstance(x, bytes) else str(x)
        dirs = [dec(d) for d in hdr["include_directory"]]
        if not v5:
            out.append(None)
        for fe in hdr["file_entry"]:
            di = fe.dir_index
            if v5:
                d = dirs[di] if di < len(dirs) else ""
            else:
                d = comp_dir if di == 0 else (dirs[di - 1] if di - 1 < len(dirs) else "")
            out.append(os.path.normpath(os.path.join(comp_dir, d, dec(fe.name))))
    cache[off] = out
    return out


def cfg_cache_key(i_path: Path) -> str:
    """TU identity = DecBench's _stripped_sha: the text after system headers are
    stripped. Two compiles of one file that differ only in predefined macros
    (e.g. -fPIC shifts the <built-in> line marker) are the same TU."""
    return _stripped_sha(i_path)


def analyse_build(bdir: Path, compiler: str, config: str, submodules: list[str], cfg_cache: dict):
    meta = json.loads((bdir / "build.json").read_text())
    work = meta["work_dir"].rstrip("/")
    tu = defaultdict(set)  # realpath src -> {i path}
    for line in (bdir / "tu_log.jsonl").read_text().splitlines():
        rec = json.loads(line)
        tu[rec["src"]].add(rec["i"])
    tu_text = {}
    for src, ipaths in tu.items():
        digests = {}
        for ip in ipaths:
            local = bdir / "i" / Path(ip).name
            if local.exists():
                digests.setdefault(cfg_cache_key(local), local)
        tu_text[src] = digests  # sha -> local path

    stats = defaultdict(int)
    per_bin: dict[str, dict] = {}
    for b in sorted(meta["binaries"], key=lambda x: x["rel"]):
        path = bdir / "bin" / b["file"]
        dw = binfmt.dwarf_info(path)
        if dw is None:
            stats["binary_no_dwarf"] += 1
            continue
        file_tables: dict = {}
        found: dict[str, list] = defaultdict(list)
        from elftools.elf.elffile import ELFFile
        with open(path, "rb") as fh:
            exec_ranges = [(s["sh_addr"], s["sh_addr"] + s["sh_size"])
                           for s in ELFFile(fh).iter_sections()
                           if s["sh_flags"] & 0x4 and s["sh_size"] and s["sh_type"] != "SHT_NOBITS"]
        for cu in dw.iter_CUs():
            top = cu.get_top_DIE()
            lang = top.attributes.get("DW_AT_language")
            if lang is None or lang.value not in C_LANGS:
                continue
            prod = binfmt.die_str_attr(top, "DW_AT_producer") or ""
            name = binfmt.die_str_attr(top, "DW_AT_name") or ""
            comp_dir = binfmt.die_str_attr(top, "DW_AT_comp_dir") or ""
            cu_src = os.path.normpath(os.path.join(comp_dir, name))
            for die in cu.iter_DIEs():
                if die.tag != "DW_TAG_subprogram" or "DW_AT_low_pc" not in die.attributes:
                    continue
                fname = binfmt.die_str_attr(die, "DW_AT_name", follow_abstract_origin=True)
                if not fname:
                    continue
                stats["dwarf_defined"] += 1
                lo = int(die.attributes["DW_AT_low_pc"].value)
                if not any(a <= lo < b for a, b in exec_ranges):
                    stats["x_low_pc_not_in_code"] += 1
                    continue
                if not producer_ok(prod, compiler, config):
                    stats["x_producer"] += 1
                    continue
                fi, owner = binfmt.die_attr_owner(die, "DW_AT_decl_file", follow_abstract_origin=True)
                if fi is None:
                    stats["x_no_decl_file"] += 1
                    continue
                files = full_file_table(dw, owner.cu, file_tables)
                if not (0 <= fi.value < len(files)) or files[fi.value] is None:
                    stats["x_no_decl_file"] += 1
                    continue
                decl = files[fi.value]
                if decl != cu_src:
                    stats["x_decl_not_primary"] += 1
                    continue
                if not cu_src.startswith(work + "/"):
                    stats["x_outside_tree"] += 1
                    continue
                rel = cu_src[len(work) + 1:]
                if any(rel == s or rel.startswith(s.rstrip("/") + "/") for s in submodules):
                    stats["x_submodule"] += 1
                    continue
                if EXCLUDED_DIRS.search(os.path.dirname(rel)):
                    stats["x_excluded_dir"] += 1
                    continue
                if os.path.basename(rel).lower() in VENDORED_BASENAMES:
                    stats["x_vendored_basename"] += 1
                    continue
                if fname in SKIP_NAMES:
                    stats["x_skip_name"] += 1
                    continue
                texts = tu_text.get(cu_src) or tu_text.get(os.path.realpath(cu_src))
                if not texts:
                    stats["x_no_preprocessed"] += 1
                    continue
                if len(texts) != 1:
                    stats["x_ambiguous_tu"] += 1
                    continue
                (sha, ipath), = texts.items()
                if sha not in cfg_cache:
                    try:
                        cfgs = extract_cfgs_from_source(ipath)
                    except Exception:  # noqa: BLE001
                        cfgs = {}
                    cfg_cache[sha] = {k: relabel_cfg(v) for k, v in cfgs.items()}
                if fname not in cfg_cache[sha]:
                    stats["x_no_source_cfg"] += 1
                    continue
                if OVERLAP is not None and overlaps(fname, cfg_cache[sha][fname]):
                    stats["x_decbench_overlap"] += 1
                    continue
                found[fname].append((int(die.attributes["DW_AT_low_pc"].value), rel, sha))
        rows = {}
        for fname, hits in found.items():
            addrs = {h[0] for h in hits}
            if len(addrs) != 1:
                stats["x_name_not_unique_in_binary"] += 1
                continue
            rows[fname] = hits[0]
        per_bin[b["file"]] = rows
    return meta, per_bin, dict(stats)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("project")
    ap.add_argument("builds", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--submodules", default="")
    ap.add_argument("--select", type=int, default=300)
    ap.add_argument("--no-strip", action="store_true")
    ap.add_argument("--overlap", type=Path, help="decbench_fingerprints.json")
    ap.add_argument("--only", default="", help="comma list of <compiler>-<config> to analyse")
    a = ap.parse_args()
    global OVERLAP
    if a.overlap:
        d = json.loads(a.overlap.read_text())
        OVERLAP = {"weak": set(d["weak"]), "strong": set(d["strong"])}
    subs = [s for s in a.submodules.split(",") if s]
    a.out.mkdir(parents=True, exist_ok=True)
    cfg_cache: dict = {}
    summary = {"project": a.project, "select": a.select, "builds": {}}
    for comp in COMPILERS:
        for conf in CONFIGS:
            key = f"{comp}-{conf}"
            if a.only and key not in a.only.split(","):
                continue
            bdir = a.builds / key
            if not (bdir / "build.json").exists():
                summary["builds"][key] = {"eligible": 0, "error": "not built"}
                continue
            meta, per_bin, stats = analyse_build(bdir, comp, conf, subs, cfg_cache)
            owner: dict[tuple[str, str], tuple[str, int, str]] = {}
            for binf in sorted(per_bin):
                for fname, (addr, rel, sha) in per_bin[binf].items():
                    owner.setdefault((rel, fname), (binf, addr, sha))

            def h(k):
                return hashlib.sha256(f"freshbench-v1/{a.project}/{k[0]}/{k[1]}".encode()).hexdigest()

            order = sorted(owner, key=h)
            chosen = order[: a.select]
            summary["builds"][key] = {
                "eligible": len(owner), "selected": len(chosen), "stats": stats,
                "build_ok": meta["success"], "build_system": meta["build_system"],
                "binaries": len(meta["binaries"]), "cc_version": meta["cc_version"],
            }
            if a.no_strip:
                continue
            odir = a.out / key
            (odir / "binaries").mkdir(parents=True, exist_ok=True)
            (odir / "stripped").mkdir(parents=True, exist_ok=True)
            by_bin: dict[str, list] = defaultdict(list)
            for k in chosen:
                binf, addr, sha = owner[k]
                by_bin[binf].append({"name": k[1], "source": k[0], "address": addr,
                                     "hash": h(k), "tu_sha256": sha})
            manifest = {"project": a.project, "compiler": comp, "config": conf,
                        "flags": meta["flags"], "cc_version": meta["cc_version"], "binaries": {}}
            for binf, funcs in sorted(by_bin.items()):
                src_bin = bdir / "bin" / binf
                dst = odir / "binaries" / binf
                dst.write_bytes(src_bin.read_bytes())
                tool = binfmt.strip_copy(src_bin, odir / "stripped" / binf,
                                         ["-R", ".comment", "-R", ".note.gnu.build-id"])
                resolved = {f["name"]: cfg_cache[f["tu_sha256"]][f["name"]] for f in funcs}
                _write_cfg_json(odir / "source_cfgs" / f"{binf}.json", key, a.project, binf,
                                resolved, "rust-joern")
                manifest["binaries"][binf] = {
                    "sha256": hashlib.sha256(dst.read_bytes()).hexdigest(),
                    "stripped_sha256": hashlib.sha256((odir / "stripped" / binf).read_bytes()).hexdigest(),
                    "strip_tool": Path(tool).name,
                    "functions": sorted(funcs, key=lambda f: f["hash"]),
                }
            # preprocessed TUs that own selected functions (GED ground-truth input)
            (odir / "sources").mkdir(exist_ok=True)
            tu_paths = {}
            for p in (bdir / "i").iterdir():
                tu_paths.setdefault(cfg_cache_key(p), p)
            for sha in sorted({owner[k][2] for k in chosen}):
                p = tu_paths[sha]
                (odir / "sources" / f"{sha[:16]}.{p.name.split('.')[0]}.i").write_bytes(p.read_bytes())
            (odir / "manifest.json").write_text(json.dumps(manifest, indent=1))
    # Qualification: the full function set (O0, both compilers) holds at least
    # N eligible functions. Optimised builds keep whatever survives, first N in
    # the same hash order (fewer only if fewer exist).
    o0 = [v for k, v in summary["builds"].items() if k.endswith("-O0")]
    ok = bool(o0) and all(b.get("eligible", 0) >= a.select for b in o0)
    summary["qualifies"] = ok
    (a.out / "prepare_summary.json").write_text(json.dumps(summary, indent=1))
    print(json.dumps({k: (v.get("eligible"), v.get("selected")) for k, v in summary["builds"].items()}))
    print("QUALIFIES" if ok else "DOES NOT QUALIFY")
    return 0


if __name__ == "__main__":
    sys.exit(main())

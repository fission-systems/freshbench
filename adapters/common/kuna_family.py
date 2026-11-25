"""Kuna and Unlace (a Kuna fork) share their JSON: one adapter for both.

Address mode: `<tool> decompile-all <bin> --json --define-function @F --addr A ...`
loads and analyzes the binary once and returns, per function,
{address, code, error, line_mappings: [{line_number, addresses}],
 variables: [{name, type, kind, arg_index, stack_offset, size, line_numbers, addresses}]}.
The tool's own line mappings and variables are passed through as evidence
(FreshBench validates every address). A requested function the document does
not hold is retried alone with `<tool> decompile <bin> <addr> --addr --json`.

List mode (--list-functions): `<tool> functions <bin> --json`, the inventory
({name, address, size}); `size` reaches the next entry or the end of the code
section, padding included. An entry with size 0 is listed only when its start
lies in an executable PT_LOAD segment, with end null.
"""
import argparse
import json
import os
import struct
import subprocess
import sys
import tempfile


def exec_segments(path):
    """[lo, hi) of every executable PT_LOAD segment of an ELF file."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:4] != b"\x7fELF":
        return []
    is64, e = data[4] == 2, "<" if data[5] == 1 else ">"
    if is64:
        (phoff,), (phentsize, phnum) = struct.unpack_from(e + "Q", data, 0x20), struct.unpack_from(e + "HH", data, 0x36)
    else:
        (phoff,), (phentsize, phnum) = struct.unpack_from(e + "I", data, 0x1C), struct.unpack_from(e + "HH", data, 0x2A)
    out = []
    for i in range(phnum):
        o = phoff + i * phentsize
        if is64:
            p_type, p_flags, _off, vaddr, _pa, _fsz, memsz = struct.unpack_from(e + "IIQQQQQ", data, o)
        else:
            p_type, _off, vaddr, _pa, _fsz, memsz, p_flags = struct.unpack_from(e + "IIIIIII", data, o)
        if p_type == 1 and p_flags & 1:  # PT_LOAD, PF_X
            out.append((vaddr, vaddr + memsz))
    return out


def as_int(v):
    if isinstance(v, int):
        return v
    if isinstance(v, str):
        return int(v, 16) if v.lower().startswith("0x") else int(v)
    return None


def evidence(f):
    """The tool's line mappings and variables, as the FreshBench contract spells them."""
    lm = []
    for m in f.get("line_mappings") or []:
        addrs = [a for a in (as_int(x) for x in m.get("addresses") or []) if a is not None]
        if m.get("line_number") is not None:
            lm.append({"line_number": int(m["line_number"]), "addresses": addrs})
    vs = []
    for v in f.get("variables") or []:
        if v.get("type") is None:
            continue
        vs.append({
            "name": v.get("name") or "",
            "type": v["type"],
            "kind": v.get("kind"),
            "arg_index": v.get("arg_index"),
            "stack_offset": v.get("stack_offset"),
            "size": v.get("size"),
            "addresses": [a for a in (as_int(x) for x in v.get("addresses") or []) if a is not None],
        })
    return {"line_mappings": lm, "variables": vs}


def records(doc):
    """address (Thumb bit cleared) -> (code or None, error, evidence)."""
    got = {}
    for f in doc.get("functions") or []:
        addr = as_int(f.get("address"))
        if addr is None:
            continue
        code = f.get("code")
        got[addr & ~1] = (code if code and code.strip() else None, f.get("error"), evidence(f))
    return got


def list_functions(tool, a):
    p = subprocess.run([tool, "functions", a.binary, "--json"], capture_output=True, text=True)
    try:
        doc = json.loads(p.stdout)
        entries = doc["functions"]
    except Exception:  # noqa: BLE001
        sys.stderr.write(f"functions exited {p.returncode}: {p.stderr[-500:]}\n")
        return 1
    if isinstance(doc, dict) and doc.get("error"):
        # Kuna exits 1 when it discovered no body, after writing the document
        sys.stderr.write(f"{doc['error']}\n")
    try:
        segs = exec_segments(a.binary)
    except Exception:  # noqa: BLE001
        segs = []
    found = {}
    for e in entries:
        start, size = as_int(e["address"]), int(e.get("size") or 0)
        if size:
            found[start] = max(found.get(start) or 0, start + size)
        elif any(lo <= start < hi for lo, hi in segs):
            found.setdefault(start, None)
    with open(a.out, "w") as out:
        for start, end in sorted(found.items()):
            out.write(json.dumps({"start": f"{start:#x}", "end": None if end is None else f"{end:#x}"}) + "\n")
    return 0


def decompile(tool, a):
    reqs = [l.strip() for l in open(a.addresses) if l.strip()]
    want = {int(r, 16) & ~1: r for r in reqs}
    got, note = {}, ""
    with tempfile.TemporaryDirectory() as td:
        # the same function starts every tool is given (Thumb bit cleared)
        df = os.path.join(td, "starts.txt")
        with open(df, "w") as f:
            f.write("".join(f"{va:#x}\n" for va in sorted(want)))
        cmd = [tool, "decompile-all", a.binary, "--json", "--define-function", "@" + df]
        for va in sorted(want):
            cmd += ["--addr", f"{va:#x}"]
        p = subprocess.run(cmd, capture_output=True, text=True)
        try:
            got = records(json.loads(p.stdout))
        except Exception:  # noqa: BLE001
            note = f" (decompile-all exited {p.returncode}: {p.stderr[-300:]})"
    with open(a.out, "w") as out:
        for va, r in sorted(want.items()):
            code, err, ev = got.get(va, (None, None, None))
            if code is None and err is None:
                q = subprocess.run([tool, "decompile", a.binary, f"{va:#x}", "--addr", "--json",
                                    "--define-function", f"{va:#x}"], capture_output=True, text=True)
                try:
                    code, err, ev = records(json.loads(q.stdout)).get(va, (None, None, None))
                    if code is None and err is None:
                        err = "no record for the address" + note
                except Exception:  # noqa: BLE001
                    err = f"decompile exited {q.returncode}: {q.stderr[-300:]}{note}"
            line = {"address": r, "code": code, "error": None if code else (str(err)[:500] if err else "no code")}
            if code and ev:
                line.update(ev)
            out.write(json.dumps(line) + "\n")
    return 0


def main(tool_env):
    ap = argparse.ArgumentParser()
    ap.add_argument("--list-functions", action="store_true")
    for k in ("binary", "addresses", "elf_base", "out"):
        ap.add_argument("--" + k.replace("_", "-"), required=k != "addresses")
    a = ap.parse_args()
    tool = os.environ[tool_env]
    if a.list_functions:
        sys.exit(list_functions(tool, a))
    if a.addresses is None:
        ap.error("--addresses is required without --list-functions")
    sys.exit(decompile(tool, a))

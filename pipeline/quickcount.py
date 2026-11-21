#!/usr/bin/env python3
"""Upper bound on eligible functions in one build (runs inside the build image).
Counts DW_TAG_subprogram with low_pc in C compile units whose primary source
lies in the project tree. prepare.py's checks only remove functions from this
set, so a count below the threshold rejects the candidate without the scorer.
usage: quickcount.py <build_dir>"""
import json, os, sys
from elftools.elf.elffile import ELFFile
b = sys.argv[1]
meta = json.load(open(os.path.join(b, "build.json")))
work = meta["work_dir"].rstrip("/") + "/"
names = set()
for bi in meta["binaries"]:
    try:
        with open(os.path.join(b, "bin", bi["file"]), "rb") as f:
            e = ELFFile(f)
            if not e.has_dwarf_info():
                continue
            dw = e.get_dwarf_info()
            for cu in dw.iter_CUs():
                top = cu.get_top_DIE()
                lang = top.attributes.get("DW_AT_language")
                if lang is None or lang.value not in (0x1, 0x2, 0xC, 0x1D, 0x2C, 0x2D):
                    continue
                g = lambda a: (top.attributes[a].value.decode(errors="replace")
                               if a in top.attributes and isinstance(top.attributes[a].value, bytes) else "")
                src = os.path.normpath(os.path.join(g("DW_AT_comp_dir"), g("DW_AT_name")))
                if not src.startswith(work):
                    continue
                for die in cu.iter_DIEs():
                    if die.tag == "DW_TAG_subprogram" and "DW_AT_low_pc" in die.attributes:
                        names.add((src, die.offset))
    except Exception as ex:  # noqa: BLE001
        print(f"skip {bi['file']}: {ex}", file=sys.stderr)
print(len(names))

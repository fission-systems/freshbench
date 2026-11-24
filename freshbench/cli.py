"""freshbench command line.

  freshbench gaps ROOT --tool fission [--json out.json] [--no-text]
      improvement targets of one tool against the others on the same rows
  freshbench bands ROOT [--json out.json]
      perfect rate and normalized GED per tool and size band
  freshbench types ROOT [--json out.json]
      type_match split into coverage (DWARF variables matched) and accuracy (matched ones right)
  freshbench determinism ROOT
      the same ground truth + identical output scored differently (must be empty)
  freshbench manifest build ROOT OUT [--projects corpus.json] [--provenance prov.json]
  freshbench manifest verify ROOT MANIFEST
  freshbench manifest summary MANIFEST
  freshbench callsites --objdump-text F --rows rows.jsonl --records records.jsonl.gz
      call-site agreement for one binary's rows (objdump of the UNSTRIPPED binary)
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
from collections import Counter, defaultdict

from . import bands as bandmod
from . import callsites, ctext, headroom, manifest, rows, scorecache, taxonomy


def _table(root: str, tool: str):
    table: dict[rows.RowKey, dict[str, headroom.Score]] = defaultdict(dict)
    raw: dict[tuple[rows.RowKey, str], dict] = {}
    for r in rows.iter_rows(root):
        k = rows.key_of(r)
        table[k][r["tool"]] = headroom.score_of(r)
        if r["tool"] == tool:
            raw[(k, r["tool"])] = {"decompiled": r.get("decompiled"), "ged": r.get("ged"),
                                   "ged_meta": r.get("ged_meta"), "abstain": r.get("abstain")}
    return table, raw


def cmd_gaps(a) -> int:
    table, raw = _table(a.root, a.tool)
    tools = sorted({t for per in table.values() for t in per})
    if a.tool not in tools:
        print(f"tool {a.tool!r} not in {tools}", file=sys.stderr)
        return 2
    tags: dict[rows.RowKey, tuple[str, ...]] = {}
    outcomes: dict[rows.RowKey, str] = {}
    mine = [k for k, per in table.items() if a.tool in per]
    code = {} if a.no_text else rows.load_code(a.root, a.tool, set(mine))
    srcmap = {} if a.no_text else rows.source_paths(a.root)
    tu_cache: dict[str, dict[str, str | None] | None] = {}
    for k in mine:
        r = raw[(k, a.tool)]
        src_f = out_f = None
        if not a.no_text and k in code:
            out_f = ctext.features(code[k])
            sp = srcmap.get(k)
            if sp is not None:
                if sp not in tu_cache:
                    try:
                        with open(os.path.join(a.root, sp), encoding="utf-8", errors="replace") as f:
                            tu_cache[sp] = ctext.definitions(f.read())
                    except OSError:
                        tu_cache[sp] = None
                defs = tu_cache[sp]
                body = defs.get(k.function) if defs else None
                if body is not None:
                    src_f = ctext.features(body, own_name=k.function)
                    out_f = ctext.features(code[k], own_name=k.function)
        c = taxonomy.classify(r, src_f, out_f)
        tags[k] = c.tags
        outcomes[k] = c.outcome
        if len(tu_cache) > 4096:
            tu_cache.clear()
    h = headroom.compute(table, a.tool, tags, outcomes, keep=a.keep)
    d = h.as_dict(a.keep)
    d["tools"] = tools
    d["text_features"] = not a.no_text
    if a.json:
        with open(a.json, "w") as f:
            json.dump(d, f, indent=1, sort_keys=True)
    print(render_gaps(d))
    return 0


def render_gaps(d: dict) -> str:
    o = d["overall"]
    lines = [f"# {d['tool']}: headroom against {', '.join(t for t in d['tools'] if t != d['tool'])}", ""]
    lines.append(f"rows {o['rows']:,}; perfect {o['perfect']:,}; reachable (another tool perfect, "
                 f"{d['tool']} not) {o['reachable']:,}; no output where another tool scored "
                 f"{o['uncovered']:,}")
    lines.append(f"GED on {d['common_rows']:,} rows both scored: {d['tool']} {d['tool_ged_on_common']:,.0f}, "
                 f"per-row best of all tools {d['virtual_best_ged_on_common']:,.0f}; gap to the best other "
                 f"tool {o['gap']:,.0f}")
    lines += ["", "| band | rows | perfect | reachable | gap | no output |", "|---|---:|---:|---:|---:|---:|"]
    for b, g in d["by_band"].items():
        lines.append(f"| {b} | {g['rows']:,} | {g['perfect']:,} | {g['reachable']:,} | {g['gap']:,.0f} | {g['uncovered']:,} |")
    if d["by_outcome"]:
        lines += ["", "| outcome | rows | reachable | gap |", "|---|---:|---:|---:|"]
        for t, g in d["by_outcome"].items():
            lines.append(f"| {t} | {g['rows']:,} | {g['reachable']:,} | {g['gap']:,.0f} |")
    if d["by_tag"]:
        lines += ["", "Shape tags (a row counts under each of its tags; totals overlap):", "",
                  "| tag | rows | reachable | gap | worst rows |", "|---|---:|---:|---:|---|"]
        for t, g in sorted(d["by_tag"].items(), key=lambda kv: -kv[1]["gap"]):
            ex = ", ".join(f"{e['row']} ({e['gap']:.0f})" for e in g["examples"][:3])
            lines.append(f"| {t} | {g['rows']:,} | {g['reachable']:,} | {g['gap']:,.0f} | {ex} |")
    lines += ["", "Similarity gaps (GED), not correctness defects."]
    return "\n".join(lines)


def cmd_bands(a) -> int:
    per = defaultdict(lambda: defaultdict(lambda: {"rows": 0, "perfect": 0, "norm": []}))
    for r in rows.iter_rows(a.root):
        meta = r.get("ged_meta") or {}
        b = bandmod.band(meta.get("source_nodes"))
        cell = per[r["tool"]][b]
        cell["rows"] += 1
        if r.get("decompiled") and r.get("ged") == 0:
            cell["perfect"] += 1
        n = bandmod.normalized_ged(r.get("ged") if r.get("decompiled") else None,
                                   meta.get("source_nodes"), meta.get("source_edges"))
        if n is not None:
            cell["norm"].append(n)
    out = {}
    lines = ["| tool | band | rows | perfect | perfect % | median normalized GED |",
             "|---|---|---:|---:|---:|---:|"]
    for tool in sorted(per):
        out[tool] = {}
        for b in bandmod.BAND_LABELS + ("unknown",):
            if b not in per[tool]:
                continue
            c = per[tool][b]
            med = statistics.median(c["norm"]) if c["norm"] else None
            out[tool][b] = {"rows": c["rows"], "perfect": c["perfect"], "median_normalized_ged": med}
            pct = 100 * c["perfect"] / c["rows"] if c["rows"] else 0
            lines.append(f"| {tool} | {b} | {c['rows']:,} | {c['perfect']:,} | {pct:.1f}% | "
                         f"{'' if med is None else f'{med:.3f}'} |")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(out, f, indent=1, sort_keys=True)
    print("\n".join(lines))
    return 0


def type_split(rows_iter) -> dict[str, dict]:
    """Per tool: coverage = matched / DWARF variables, accuracy = right / matched.

    From DecBench's type_match_meta (tp, fp, fn; tp + fp + fn = gt_vars). Micro
    averages over rows that have type ground truth. A tool can raise its
    type_match score by matching fewer variables; the split shows which moved.
    """
    acc = defaultdict(lambda: {"rows": 0, "perfect": 0, "gt_vars": 0, "matched": 0, "right": 0})
    for r in rows_iter:
        m = r.get("type_match_meta") or {}
        if "gt_vars" not in m or not m.get("gt_vars"):
            continue
        c = acc[r["tool"]]
        c["rows"] += 1
        c["perfect"] += bool(r.get("type_match_perfect"))
        c["gt_vars"] += m["gt_vars"]
        c["matched"] += m.get("tp", 0) + m.get("fp", 0)
        c["right"] += m.get("tp", 0)
    out = {}
    for tool in sorted(acc):
        c = acc[tool]
        out[tool] = dict(c, coverage=c["matched"] / c["gt_vars"] if c["gt_vars"] else None,
                         accuracy=c["right"] / c["matched"] if c["matched"] else None)
    return out


def cmd_types(a) -> int:
    out = type_split(rows.iter_rows(a.root))
    lines = ["| tool | rows with types | perfect | DWARF vars | matched | coverage | accuracy of matched |",
             "|---|---:|---:|---:|---:|---:|---:|"]
    for tool, c in out.items():
        cov = "" if c["coverage"] is None else f"{100 * c['coverage']:.1f}%"
        accu = "" if c["accuracy"] is None else f"{100 * c['accuracy']:.1f}%"
        lines.append(f"| {tool} | {c['rows']:,} | {c['perfect']:,} | {c['gt_vars']:,} | {c['matched']:,} | {cov} | {accu} |")
    if a.json:
        with open(a.json, "w") as f:
            json.dump(out, f, indent=1, sort_keys=True)
    print("\n".join(lines))
    return 0


def cmd_determinism(a) -> int:
    unstable = []

    def tap(it):
        for r in it:
            if r.get(f"{a.metric}_unstable"):
                unstable.append(rows.key_of(r).as_id() + f" [{r['tool']}]")
            yield r

    bad = scorecache.same_text_disagreements(tap(rows.iter_rows(a.root)), a.metric)
    for rid, vals in bad[: a.limit]:
        print(f"{rid}\t{vals}")
    for rid in sorted(unstable)[: a.limit]:
        print(f"{rid}\trescored differently")
    print(f"{len(bad)} (ground truth, output) pairs with more than one {a.metric} score; "
          f"{len(unstable)} rows changed on rescore")
    return 1 if bad or unstable else 0


def cmd_manifest(a) -> int:
    if a.action == "build":
        projects = None
        if a.projects:
            with open(a.projects) as f:
                pj = json.load(f)
            items = pj.get("projects", pj) if isinstance(pj, dict) else pj
            projects = {p["slug"]: p for p in items} if isinstance(items, list) else items
        prov = None
        if a.provenance:
            with open(a.provenance) as f:
                prov = {tuple(k.split("|", 1)): v for k, v in json.load(f).items()}
        entries = manifest.build_manifest(a.root, prov, projects)
        manifest.write_manifest(entries, a.out)
        print(json.dumps(manifest.summarize(entries), indent=1))
        return 0
    if a.action == "verify":
        problems = manifest.verify(a.root, manifest.read_manifest(a.manifest))
        for p in problems[:200]:
            print(p)
        print(f"{len(problems)} problems")
        return 1 if problems else 0
    print(json.dumps(manifest.summarize(manifest.read_manifest(a.manifest)), indent=1))
    return 0


def cmd_callsites(a) -> int:
    with open(a.objdump_text, encoding="utf-8", errors="replace") as f:
        funcs = callsites.parse_objdump(f.read())
    addr2name = callsites.symbol_table(funcs)
    code = {}
    for r in rows.iter_jsonl(a.records):
        code[(r["binary"], r["function"])] = r.get("code") or ""
    n = 0
    for r in rows.iter_jsonl(a.rows):
        if a.binary and r["binary"] != a.binary:
            continue
        fc = funcs.get(r.get("address"))
        rec = {k: r.get(k) for k in ("project", "compiler", "config", "binary", "function", "tool")}
        if fc is None:
            rec["callsites"] = None
            rec["callsites_skip"] = "no function at address in the objdump"
        elif not r.get("decompiled"):
            rec["callsites"] = None
            rec["callsites_skip"] = "no output"
        else:
            out = ctext.features(code.get((r["binary"], r["function"]), ""), own_name=r["function"]).calls
            rec["callsites"] = callsites.compare(fc.calls, out, addr2name, a.delta).as_dict()
        print(json.dumps(rec, sort_keys=True))
        n += 1
    print(f"{n} rows", file=sys.stderr)
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="freshbench", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("gaps")
    g.add_argument("root")
    g.add_argument("--tool", required=True)
    g.add_argument("--json")
    g.add_argument("--keep", type=int, default=5)
    g.add_argument("--no-text", action="store_true", help="skip source/output text features")
    b = sub.add_parser("bands")
    b.add_argument("root")
    b.add_argument("--json")
    ty = sub.add_parser("types")
    ty.add_argument("root")
    ty.add_argument("--json")
    d = sub.add_parser("determinism")
    d.add_argument("root")
    d.add_argument("--metric", default="ged")
    d.add_argument("--limit", type=int, default=50)
    m = sub.add_parser("manifest")
    m.add_argument("action", choices=["build", "verify", "summary"])
    m.add_argument("root", nargs="?")
    m.add_argument("out_or_manifest", nargs="?")
    m.add_argument("--projects")
    m.add_argument("--provenance")
    c = sub.add_parser("callsites")
    c.add_argument("--objdump-text", required=True)
    c.add_argument("--rows", required=True)
    c.add_argument("--records", required=True)
    c.add_argument("--binary")
    c.add_argument("--delta", type=lambda s: int(s, 0), default=0)
    a = ap.parse_args(argv)
    if a.cmd == "manifest":
        if a.action == "build":
            a.out = a.out_or_manifest
        elif a.action == "verify":
            a.manifest = a.out_or_manifest
        else:
            a.manifest = a.root
    return {"gaps": cmd_gaps, "bands": cmd_bands, "types": cmd_types, "determinism": cmd_determinism,
            "manifest": cmd_manifest, "callsites": cmd_callsites}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())

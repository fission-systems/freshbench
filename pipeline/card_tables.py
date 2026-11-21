#!/usr/bin/env python3
"""Markdown tables for the dataset card from summary.json + analysis.json (+ DecBench reference).
usage: card_tables.py <summary.json> <analysis.json>"""
import json, sys
S = json.load(open(sys.argv[1])); A = json.load(open(sys.argv[2]))
sl = S["slices"]; tools = ["kuna", "angr", "fission", "ghidra"]
NAME = {"kuna": "Kuna 1.1003", "angr": "angr 10.0.7", "fission": "Fission 18f9e3d", "ghidra": "Ghidra 12.2"}


def row(s, t):
    d = sl[s][t]
    return d


out = []
out.append("| rank | tool | Union(GED,type) | 95% CI (project bootstrap) | GED perfect | GED total distance (48,810 common rows) | type perfect | type mean (common rows) | decompiled |")
out.append("|---:|---|---:|---|---:|---:|---:|---:|---:|")
order = sorted(tools, key=lambda t: -sl["all"][t]["union"])
for i, t in enumerate(order, 1):
    d = sl["all"][t]; ci = A["union_ci95"][t]
    out.append(f"| {i} | {NAME[t]} | {d['union']:,} ({d['union_pct']:.1f}%) | {ci[0]:.1f}-{ci[1]:.1f}% | {d['ged_perfect']:,} | "
               f"{d['ged_distance_common']:,} | {d['type_perfect']:,} / {d['type_scored']:,} | {d['type_common_mean']:.3f} | "
               f"{d['decompiled']:,} / {d['rows']:,} ({100*d['decompiled']/d['rows']:.1f}%) |")
print("\n".join(out)); print()
for title, keys in [("Per optimization level (GCC + Clang)", [f"config={c}" for c in ["O0", "O1", "O2", "O3", "Os", "O2-noinline"]]),
                    ("Per compiler", ["compiler=gcc", "compiler=clang"]),
                    ("Per build", [f"build={c}-{o}" for c in ("gcc", "clang") for o in ["O0", "O1", "O2", "O3", "Os", "O2-noinline"]])]:
    print(f"#### {title}\n")
    print("| slice | rows | " + " | ".join(f"{NAME[t]} Union / GED perfect / GED dist." for t in tools) + " |")
    print("|---|---:|" + "---:|" * len(tools))
    for k in keys:
        cells = [f"{sl[k][t]['union_pct']:.1f}% / {sl[k][t]['ged_perfect']:,} / {sl[k][t]['ged_distance_common']:,}" for t in tools]
        best = max(tools, key=lambda t: sl[k][t]["union"])
        cells = [f"**{c}**" if t == best else c for c, t in zip(cells, tools)]
        print(f"| {k.split('=')[1]} | {sl[k][tools[0]]['rows']:,} | " + " | ".join(cells) + " |")
    print()
# DecBench reference (local sweep det-fix-f21379e1b, Fission-scorable rows; rivals = DecBench published runs)
dec = {"unoptimized (gcc O0)": ({"kuna": 17443, "angr": 16026, "fission": 15580, "ghidra": 12636},
                                {"kuna": 201416, "angr": 238871, "fission": 335795, "ghidra": 303380}, "build=gcc-O0"),
       "optimized (gcc O2 -fno-inline)": ({"kuna": 12705, "angr": 12607, "fission": 12337, "ghidra": 10734},
                                           {"kuna": 295281, "angr": 373519, "fission": 421949, "ghidra": 333163}, "build=gcc-O2-noinline"),
       "inlined (gcc O2)": ({"kuna": 8115, "angr": 8316, "fission": 8028, "ghidra": 6608},
                            {"kuna": 412017, "angr": 454590, "fission": 494198, "ghidra": 455975}, "build=gcc-O2"),
       "full": ({"kuna": 38263, "angr": 36949, "fission": 35945, "ghidra": 29978},
                {"kuna": 908714, "angr": 1066980, "fission": 1251942, "ghidra": 1092518}, "all")}
print("#### Relative to Kuna: DecBench vs FreshBench\n")
print("| config | metric | DecBench angr | FreshBench angr | DecBench Fission | FreshBench Fission | DecBench Ghidra | FreshBench Ghidra |")
print("|---|---|---:|---:|---:|---:|---:|---:|")
for name, (u, g, fk) in dec.items():
    fu = {t: sl[fk][t]["union"] for t in tools}; fg = {t: sl[fk][t]["ged_distance_common"] for t in tools}
    print(f"| {name} | Union / Kuna's | " + " | ".join(f"{u[t]/u['kuna']:.3f} | {fu[t]/fu['kuna']:.3f}" for t in ["angr", "fission", "ghidra"]) + " |")
    print(f"| {name} | GED distance / Kuna's | " + " | ".join(f"{g[t]/g['kuna']:.3f} | {fg[t]/fg['kuna']:.3f}" for t in ["angr", "fission", "ghidra"]) + " |")
print()
print("#### Union per project\n")
print("| project | " + " | ".join(NAME[t] for t in tools) + " |"); print("|---|" + "---:|" * len(tools))
for p, v in A["per_project_union_pct"].items():
    best = max(tools, key=lambda t: v[t])
    print(f"| {p.replace('__', '/')} | " + " | ".join((f"**{v[t]:.1f}%**" if t == best else f"{v[t]:.1f}%") for t in tools) + " |")
print()
print("#### Pairwise Union difference (project bootstrap, 10,000 resamples)\n")
print("| pair | difference (pp) | 95% CI | P(first > second) |"); print("|---|---:|---|---:|")
for k, v in A["pairs"].items():
    print(f"| {k} | {v['point']:+.2f} | {v['ci95'][0]:+.2f} .. {v['ci95'][1]:+.2f} | {v['p_a_greater']:.3f} |")
print()
print("#### Runs (GitHub Actions ubuntu-24.04, 4 vCPU / 16 GB, decompile step only)\n")
print("| tool | binaries | functions | with output | wall s | CPU s | peak RSS MB | binary timeouts | OOM | errors |")
print("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
for t in tools:
    f = S["runs"][t]
    print(f"| {NAME[t]} | {int(f['binaries'])} | {int(f['functions']):,} | {int(f['decompiled']):,} | {f['wall_s']:,.0f} | {f['cpu_s']:,.0f} | "
          f"{f['maxrss_kb']/1024:,.0f} | {int(f.get('binary_timeouts',0))} | {int(f.get('binary_oom',0))} | {int(f.get('binary_errors',0))} |")

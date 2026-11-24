#!/usr/bin/env python3
"""Dataset card (README.md) for the FreshBench release from DecBench aggregates.

usage: card100.py <decbench_site_out_dir> <corpus.json> <dataset_dir> <funnel.json>
Reads <out>/summary.md (decbench.rendering.aggregate tables), <out>/analysis.json (project bootstrap).
"""
import json, sys
from pathlib import Path
out, corpus, ds, funnel = Path(sys.argv[1]), json.loads(Path(sys.argv[2]).read_text()), Path(sys.argv[3]), json.loads(Path(sys.argv[4]).read_text())
summary = (out / "summary.md").read_text()
A = json.loads((out / "analysis.json").read_text())
man = json.loads((ds / "configs/full/manifest.json").read_text())
sections = {s.split("\n", 1)[0]: s.split("\n", 1)[1] for s in summary.split("\n## ")[1:]}
NAME = {"borer": "borer 0.3.1", "kuna": "Kuna 1.1003", "angr": "angr 10.0.7", "fission": "Fission 18f9e3d", "ghidra": "Ghidra 12.2"}
order = sorted(A["tools"], key=lambda t: -A["union_pct"][t])
n_proj = len(corpus["projects"])
L = []
L += ["---", "pretty_name: FreshBench", "license: other", "license_name: mixed-osi",
      'license_details: "Each project keeps its own OSI license; see projects/<project>/."',
      "tags:", "- decompilation", "- binary-analysis", "- reverse-engineering", "- benchmark",
      "size_categories:", "- 100K<n<1M", "---", "",
      "# FreshBench", "",
      f"A DecBench-style C decompilation benchmark on a corpus that **no decompiler was tuned on**: {n_proj} GitHub C",
      "projects chosen by a fixed rule (none of DecBench's 40 projects, no well-known projects), each built with the",
      "latest GCC and Clang at six optimization settings, up to 300 functions per build:",
      f"**{man['function_count']:,} function rows** in {man['binary_count']:,} x86-64 Linux ELF binaries. Ground truth is",
      "rust-joern source CFGs (GED), DWARF types (type_match) and recompilation with the original toolchain",
      "(byte_match). The outputs and per-function scores of borer, angr, Kuna, Fission and Ghidra were",
      "measured through DecBench's own drivers and scorer, and every number below comes from DecBench's own",
      "aggregation (`decbench.rendering.aggregate`).", "",
      "Code, CI and the full build/decompile pipeline: <https://github.com/sjkim1127/freshbench>.",
      "Static analysis only: no corpus binary was ever executed.", "",
      "## Results", "",
      "Union = perfect on at least one of GED, type_match, byte_match. Each metric has one denominator shared by",
      "every tool (the functions that metric can measure), and a tool with no output for a function counts as a",
      "miss. errors = functions a tool produced no output for; compiles = byte_match's fix-up loop recompiled",
      "the output.", "",
      sections["full"].strip(), "",
      f"Project bootstrap (10,000 resamples of the {n_proj} projects), Union:", "",
      "| tool | Union | 95% interval | ranked first |", "|---|---:|---:|---:|"]
for t in order:
    L.append(f"| {NAME[t]} | {A['union_pct'][t]:.1f}% | {A['union_ci95'][t][0]:.1f}-{A['union_ci95'][t][1]:.1f}% | {100*A['rank_freq'][t].get('1',0):.2f}% |")
L += ["", "Pairwise Union differences (95% interval, project bootstrap):", "",
      "| pair | difference | 95% interval | P(first > second) |", "|---|---:|---:|---:|"]
for i, a in enumerate(order):
    for b in order[i + 1:]:
        k = f"{a}-{b}" if f"{a}-{b}" in A["pairs"] else f"{b}-{a}"
        p = A["pairs"][k]; sgn = 1 if k == f"{a}-{b}" else -1
        lo, hi = sorted((sgn * p["ci95"][0], sgn * p["ci95"][1]))
        pg = p["p_a_greater"] if sgn == 1 else 1 - p["p_a_greater"]
        L.append(f"| {NAME[a]} - {NAME[b]} | {sgn*p['point']:+.2f} pp | {lo:+.2f} to {hi:+.2f} | {pg:.3f} |")
for name in ["gcc", "clang", "O0", "O1", "O2", "O3", "Os", "O2-noinline"]:
    L += ["", f"### {name}", "", sections[name].strip()]
L += ["", "### Pipeline health", "", sections["pipeline health"].strip(), "",
      "### Cost", "", sections["cost (batch wall / function, per binary)"].strip(), "",
      "Batch wall time per function on GitHub Actions `ubuntu-24.04` runners (4 vCPU, 16 GB), one job per",
      "(project, tool, compiler); not a controlled speed benchmark.", ""]
L += ["### Caveats", "",
      "* **type_match is not comparable across two groups.** DecBench scores angr, Ghidra and Kuna with",
      "  instruction-address correspondence (`ADDRESS_CORRESPONDENCE_BACKENDS`); borer and Fission are outside",
      "  that list at the pinned commit and go through the legacy text path (`parse_c_variables`).",
      "* **borer has no DecBench driver.** `pipeline/borer_raw.py` is a FreshBench-side backend",
      "  (`borer export <binary> --c out.c --library`, split per function; functions over the export's",
      "  4096-block budget decompiled alone). Release `borer-v0.3.1-x86_64-unknown-linux-gnu.tar.gz`,",
      "  sha256 `d3b7f894edf9703e6e3c128332a62c9acdb49d6e2ce9ceb2e1aab0fec3845d53`, AGPL-3.0.",
      "* **byte_match on Clang builds.** DecBench's fix-up loop parses GCC diagnostics only, so on Clang builds the",
      "  loop runs on GCC 16.2.0 diagnostics and the compared object is built with Clang 23.1.6",
      "  (`pipeline/score_bytematch.py`). DecBench itself recompiles every ELF with `gcc`.",
      "* **Ghidra produced no function for 27.4% of the targets**: its default auto-analysis disassembles many",
      "  targets (often callbacks reached only through function-pointer tables) without creating a function.",
      "* **exonerate, angr:** six builds (gcc O2/O3, clang O1/O2/O3/O2-noinline) exceeded the 350-minute job limit",
      "  twice, once per job and once per configuration. Their 1,800 functions are recorded as angr failures, as",
      "  DecBench counts a binary timeout.",
      "* **One binary excluded:** MisraStdC's `Tests/.debug/scd.debug` is a separate debug-info file",
      "  (`objcopy --only-keep-debug`, `.text` is `SHT_NOBITS`); no tool had code to decompile. Selection now",
      "  requires a function's code to be in the file.",
      "* **The corpus has its own bias.** The rule favours projects with a simple, offline build and at least 300",
      "  functions of their own: emulators, language implementations, network/system tools, DSP and media",
      "  libraries. Test, example, benchmark and vendored directories are excluded.",
      "* Local measurements, not an official DecBench ranking.", ""]
L += ["## Projects", "", "| project | license | stars | build | min eligible / build |", "|---|---|---:|---|---:|"]
for p in sorted(corpus["projects"], key=lambda p: p["slug"].lower()):
    el = p.get("eligible") or {}
    bs = p.get("build_system", "")
    L.append(f"| [{p['repo']}]({p.get('html_url','https://github.com/'+p['repo'])}) @ `{p['sha'][:10]}` | {p.get('license')} | {p.get('stars')} | {bs} | {min(el.values()) if el else ''} |")
L += ["", "## How the corpus was built", "",
      "**Selection rule** (fixed before any build; code in the GitHub repo `selection/`):", "",
      "1. Every GitHub repository with primary language C, 50-3,000 stars, a push on or after 2024-11-22, not a",
      "   fork, not archived, not a template (14,120 repositories, snapshot 2026-11-22).",
      "2. OSI-approved license (SPDX id) and at most 200 MB: 8,140 left.",
      "3. Excluded by name: DecBench's 40 projects and a fixed list of well-known projects: 7,773 left.",
      "4. Candidates are tried in the order of `sha256(\"freshbench-v1/\" + full_name.lower())`. A candidate",
      "   qualifies when it builds offline in the fixed build image and both O0 builds have at least 300 eligible",
      f"   functions. The first {n_proj} qualifying candidates are the corpus: {funnel['tried']:,} candidates were tried",
      "   (" + ", ".join(f"{v:,} {k}" for k, v in funnel["counts"].items() if k != "qualifies") + ").", "",
      "**Builds.** x86-64 Linux ELF, GCC 16.2.0 (`gcc:16.2.0-trixie`) and Clang/LLVM 23.1.6, each at `-O0 -O1 -O2",
      "-O3 -Os` and `-O2 -fno-inline`: 12 builds per project. A compiler wrapper removes the project's own `-O*`,",
      "`-g*`, inlining, LTO, sanitizer and strip flags and appends `-g -fno-builtin` (DecBench's base flags) plus the",
      "build's flags; a project that names no `-std=` gets `-std=gnu17`.", "",
      "**Functions (rows).** A function is eligible in a build when it is a `DW_TAG_subprogram` whose `low_pc` lies in",
      "an executable section with bytes in the file, its compile unit is C, it is declared in the unit's own source",
      "file inside the project tree (not a header, submodule, third-party/test/example/benchmark/fuzz directory or",
      "well-known single-file library), one preprocessed text exists for the file, rust-joern yields a CFG for it,",
      "its name occurs at one address only, and it does not overlap DecBench. Per build, the first 300 eligible",
      "functions in `sha256(\"freshbench-v1/<project>/<source path>/<name>\")` order are kept.", "",
      "**Ground truth and scoring.** Source CFGs come from the preprocessed translation unit through DecBench's",
      "`extract_cfgs_from_source` (rust-joern `6b85a04`); types from the unstripped binary's DWARF; byte_match",
      "recompiles each output with the build's own compiler and flags. Decompilers see the stripped copy only.",
      "Every tool runs through DecBench's run driver (`_timed_decompile`, 3,600 s per binary, 600 s per function,",
      "12 GiB) and is scored with DecBench pinned to Noelo-Lab/decbench PR #129 head `d09823c`.", "",
      "| tool | version | how |", "|---|---|---|",
      "| borer | 0.3.1 (release, linux-x86_64) | FreshBench-side backend `pipeline/borer_raw.py` |",
      "| angr | 10.0.7 (PyPI) | DecBench `angr` raw backend |",
      "| Kuna | 1.1003 (release, linux-x86_64) | DecBench `kuna` raw backend |",
      "| Fission | main `18f9e3d` | DecBench `fission` raw backend (PR #129) |",
      "| Ghidra | 12.2, Temurin JDK 21, pyghidra | DecBench `ghidra` raw backend |", "",
      "## Layout", "",
      "| path | content |", "|---|---|",
      "| `binaries/<build>/<project>/<binary>` | unstripped `-g` binaries: ground truth only |",
      "| `stripped/<build>/<project>/<binary>` | the copies every decompiler was given |",
      "| `sources/<project>/<sha16>.<stem>.c` | header-stripped preprocessed TUs that own the scored functions |",
      "| `pipeline_data/source_cfgs/<build>/<project>/<binary>.json` | rust-joern source CFGs (DecBench format) |",
      "| `configs/<config>/manifest.json` | `full`, `gcc`, `clang`, `O0`..`O2-noinline`, `<compiler>-<opt>` |",
      "| `results/function_results.jsonl` | one row per (function, tool): GED, type_match, byte_match, perfect flags, output hash |",
      "| `results/<tool>/<build>/<project>/` | decompiled code (`records.jsonl.gz`), per-binary timing/failures (`run.json`) |",
      "| `results/summary.md`, `results/analysis.json` | the tables above and the project bootstrap |",
      "| `projects/<project>/` | upstream license file(s) and source repository/commit (`source.json`) |", "",
      "`<build>` is `<compiler>-<opt>`: `gcc|clang` x `O0 O1 O2 O3 Os O2-noinline`.", ""]
(ds / "README.md").write_text("\n".join(L))
print("README lines", len(L))

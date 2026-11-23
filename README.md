# FreshBench

A DecBench-style C decompilation benchmark built from GitHub C projects that
share nothing with DecBench, so no decompiler has been tuned on it.

* **Selection rule** (`selection/enumerate.py`): GitHub primary language C,
  50-3000 stars, pushed within 2 years of 2026-11-22, not a fork / archived /
  template, OSI license, <= 200 MB; DecBench's 40 projects and well-known
  projects excluded by name (`selection/exclude.py`). Candidates are tried in
  sha256 order of their name; a candidate qualifies if it builds offline in the
  fixed image (`build/Dockerfile`) and has >= 300 eligible functions in every
  one of the 12 builds. The first 20 qualifying candidates form the corpus.
* **Builds**: GCC 16.2.0 and Clang/LLVM 23.1.6, x86-64 Linux ELF, each at
  O0, O1, O2, O3, Os, O2 -fno-inline; `-g -fno-builtin` (DecBench's base
  flags) pinned by a compiler wrapper (`build/fbcc.py`).
* **Rows** (`pipeline/prepare.py`): 300 functions per (project, build) in
  sha256 order among eligible functions; DecBench overlap removed by CFG
  fingerprint (`overlap/`).
* **Ground truth**: rust-joern source CFGs from the preprocessed TU (GED) and
  DWARF (type_match), scored with DecBench (`ci/pins.env`).
* **Decompilers** see stripped copies only; Ghidra, angr, Kuna and Fission run
  through DecBench's raw backends (`pipeline/run_tool.py`).

Static analysis only: no corpus binary is ever executed.

## Results (2026-11-22)

Dataset (public): https://huggingface.co/datasets/sjkim1127/freshbench — 20 projects,
662 binaries, 71,783 function rows, all four tools' outputs and per-function scores.

| rank | tool | Union(GED,type) | GED perfect | GED total distance (common rows) | output |
|---:|---|---:|---:|---:|---:|
| 1 | Kuna 1.1003 | 39.8% | 26,428 | 1,165,279 | 100% |
| 2 | angr 10.0.7 | 39.2% | 26,069 | 1,136,845 | 99.9% |
| 3 | Fission 18f9e3d | 37.6% | 24,809 | 1,647,593 | 100% |
| 4 | Ghidra 12.2 | 22.7% | 14,870 | 1,269,247 | 69.1% |

Full tables, caveats and methodology: the dataset card. Pipeline: `selection/` -> `ci/qualify.sh`
(build + `pipeline/prepare.py`) -> `pipeline/run_tool.py` (DecBench driver + scorer) ->
`pipeline/report.py`, `pipeline/analysis.py` -> `pipeline/assemble.py`, `pipeline/finalize_dataset.py`.

### borer: 2-build probe, not a ranking

[borer](https://github.com/imjustprism/borer) v0.3.1 (AGPL-3.0) was added at a user's request. It has
been measured so far on **one project, two builds** (simavr, `gcc -O0` and `gcc -O2`, 600 rows); the run
over the whole corpus is queued (GitHub Actions run 45066341615) and will replace this section.
Do not read the probe as a ranking: one project cannot carry a project-bootstrap interval.

* Binary: the release `borer-v0.3.1-x86_64-unknown-linux-gnu.tar.gz`, sha256
  `d3b7f894edf9703e6e3c128332a62c9acdb49d6e2ce9ceb2e1aab0fec3845d53` (`ci/pins.env`). Run on CI only.
* DecBench has no borer driver. `pipeline/borer_raw.py` is a FreshBench-side backend registered into
  the pinned DecBench checkout: `borer export <binary> --c out.c --library` once per binary, split per
  function on the `// <name> @ 0x<addr>` headers; functions over the export's 4096-block budget are
  decompiled alone with `borer decompile`. Everything after that (timeouts, relabel, GED, type_match,
  byte_match) is the same DecBench path as the other tools.
* **Types are not comparable with Ghidra, angr and Kuna.** borer emits no variable or line-address
  records, so type_match scores it on DecBench's legacy text path (`parse_c_variables`), the same path
  Fission is scored on; Ghidra, angr and Kuna use address correspondence.
* Probe run: GitHub Actions 45066235329. The other tools' numbers are from the same 600 rows of the
  main runs (byte_match rerun 45040633460).


**simavr gcc-O0** (300 rows; GED distance over the 256 rows every tool has a GED for)

| tool | output | GED perfect | GED distance (common) | recompiles | byte_match perfect | type perfect* | wall s/fn |
|---|---:|---:|---:|---:|---:|---:|---:|
| borer 0.3.1 | 300 | 175 | 1,709 | 143 | 16 | 6 | 0.005 |
| Kuna 1.1003 | 300 | 154 | 1,612 | 235 | 6 | 6 | 0.047 |
| angr 10.0.7 | 300 | 144 | 1,922 | 137 | 23 | 6 | 0.277 |
| Fission 18f9e3d | 300 | 137 | 2,405 | 256 | 21 | 6 | 0.044 |
| Ghidra 12.2 | 267 | 121 | 2,199 | 157 | 5 | 4 | 0.211 |

**simavr gcc-O2** (300 rows; GED distance over the 177 rows every tool has a GED for)

| tool | output | GED perfect | GED distance (common) | recompiles | byte_match perfect | type perfect* | wall s/fn |
|---|---:|---:|---:|---:|---:|---:|---:|
| borer 0.3.1 | 300 | 89 | 3,035 | 118 | 54 | 1 | 0.005 |
| Kuna 1.1003 | 300 | 82 | 2,698 | 194 | 35 | 2 | 0.036 |
| angr 10.0.7 | 300 | 84 | 3,071 | 91 | 25 | 1 | 0.359 |
| Fission 18f9e3d | 300 | 81 | 2,975 | 234 | 23 | 1 | 0.042 |
| Ghidra 12.2 | 191 | 57 | 2,722 | 105 | 25 | 1 | 0.140 |

\* legacy text path for borer and Fission, address correspondence for the others: not comparable
across those two groups.

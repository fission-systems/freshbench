# FreshBench

A DecBench-style C decompilation benchmark built from GitHub C projects that
share nothing with DecBench, so no decompiler has been tuned on it.

* **Selection rule** (`selection/enumerate.py`): GitHub primary language C,
  50-3000 stars, pushed within 2 years of 2026-11-22, not a fork / archived /
  template, OSI license, <= 200 MB; DecBench's 40 projects and well-known
  projects excluded by name (`selection/exclude.py`). Candidates are tried in
  sha256 order of their name; a candidate qualifies if it builds offline in the
  fixed image (`build/Dockerfile`) and has >= 300 eligible functions in every
  one of the 12 builds. The first 100 qualifying candidates form the corpus (1,859 tried; the first release used the first 20).
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

## Results (2026-11-24, 100 projects)

Dataset (public): https://huggingface.co/datasets/sjkim1127/freshbench — 100 projects, 10,388 binaries,
354,167 function rows, five tools' outputs and per-function scores. Every number is computed by DecBench's own
aggregation (`decbench.rendering.aggregate`, via `pipeline/decbench_site.py`): one denominator per metric shared
by every tool, a missing output counts as a miss, Union = perfect on at least one of GED, type_match, byte_match.

| tool | Union | ged | type_match | byte_match | errors | compiles |
|---|---:|---:|---:|---:|---:|---:|
| borer 0.3.1 | 44.9% (159,181/354,146) | 40.4% (142,110/351,660) | 7.4% (17,731/239,276) | 12.3% (43,397/354,146) | 0.1% (514/354,167) | 58.7% (207,584/353,653) |
| angr 10.0.7 | 42.6% (150,727/354,146) | 36.8% (129,255/351,660) | 7.5% (17,956/239,276) | 11.3% (40,185/354,146) | 0.7% (2,409/354,167) | 80.0% (281,575/351,758) |
| kuna 1.1003 | 42.5% (150,377/354,146) | 37.1% (130,558/351,660) | 9.0% (21,559/239,276) | 11.5% (40,814/354,146) | 1.8% (6,497/354,167) | 78.1% (271,658/347,670) |
| fission 18f9e3d | 39.5% (140,039/354,146) | 34.9% (122,693/351,660) | 7.5% (17,956/239,276) | 8.1% (28,847/354,146) | 0.0% (41/354,167) | 82.8% (293,214/354,126) |
| ghidra 12.2 | 26.4% (93,606/354,146) | 22.2% (77,925/351,660) | 5.3% (12,603/239,276) | 7.9% (28,126/354,146) | 27.4% (96,933/354,167) | 85.0% (218,761/257,234) |

distance (mean · median · at-0 / measured)

| tool | ged | type_match | byte_match |
|---|---|---|---|
| borer | 22.6 · med 5 · 142,110/350,929 | 3.5 · med 2 · 17,731/238,885 | 39.7 · med 11 · 43,397/207,584 |
| angr | 21.0 · med 5 · 129,255/346,025 | 3.3 · med 2 · 17,956/238,133 | 64.1 · med 20 · 40,185/281,575 |
| kuna | 22.2 · med 5 · 130,558/345,197 | 3.1 · med 2 · 21,559/234,603 | 84.4 · med 18 · 40,814/271,658 |
| fission | 28.7 · med 6 · 122,693/351,226 | 3.6 · med 2 · 17,956/239,262 | 77.0 · med 24 · 28,847/293,214 |
| ghidra | 25.0 · med 7 · 77,925/255,222 | 3.6 · med 2 · 12,603/180,573 | 82.7 · med 23 · 28,126/218,761 |

Project bootstrap (10,000 resamples of the 100 projects), Union:

| tool | Union | 95% interval | ranked first |
|---|---:|---:|---:|
| borer 0.3.1 | 44.9% | 42.6-47.5% | 100.00% |
| angr 10.0.7 | 42.6% | 40.1-45.2% | 0.00% |
| Kuna 1.1003 | 42.5% | 39.8-45.2% | 0.00% |
| Fission 18f9e3d | 39.5% | 37.0-42.2% | 0.00% |
| Ghidra 12.2 | 26.4% | 24.3-28.7% | 0.00% |

Per-compiler and per-optimization tables, pipeline health, cost and caveats: the dataset card. Caveats in short:
type_match for borer and Fission uses DecBench's legacy text path (not comparable with the address-based scores
of angr, Ghidra and Kuna); borer runs through a FreshBench-side backend (`pipeline/borer_raw.py`, v0.3.1,
AGPL-3.0, CI only); byte_match on Clang builds drives DecBench's fix-up loop with GCC diagnostics and builds the
compared object with Clang; six exonerate builds exceeded angr's job limit and count as angr failures; one
debug-info-only binary (MisraStdC `scd.debug`) is excluded.

Pipeline: `selection/` -> `ci/qualify.sh` (build + `pipeline/prepare.py`) -> `pipeline/run_tool.py` (DecBench
driver + scorer, byte_match in the score image) -> `pipeline/decbench_site.py`, `pipeline/analysis.py` ->
`pipeline/assemble.py`, `pipeline/finalize_dataset.py`, `pipeline/card100.py`. Corpus list:
`selection/corpus100.json`.

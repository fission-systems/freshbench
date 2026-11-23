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

### Five tools with byte_match, first 20 projects (2026-11-23)

The 20 projects above re-scored with all three DecBench metrics (GED, type_match, byte_match) and with
[borer](https://github.com/imjustprism/borer) v0.3.1 (AGPL-3.0) added. Every number below is computed by
DecBench's own aggregation (`decbench.rendering.aggregate`, via `pipeline/decbench_site.py`): shared
per-metric denominators, a missing output counts as a miss, Union = perfect on at least one metric.
The 100-project corpus replaces this when its runs finish.

| tool | Union | ged | type_match | byte_match | errors | compiles |
|---|---:|---:|---:|---:|---:|---:|
| borer 0.3.1 | 45.2% (32,425/71,783) | 40.7% (28,766/70,665) | 6.9% (3,257/46,997) | 11.9% (8,516/71,783) | 0.0% (0/71,783) | 60.0% (43,101/71,783) |
| kuna 1.1003 | 42.8% (30,711/71,783) | 37.4% (26,428/70,665) | 9.1% (4,276/46,997) | 11.4% (8,205/71,783) | 0.0% (0/71,783) | 77.8% (55,880/71,783) |
| angr 10.0.7 | 42.7% (30,622/71,783) | 36.9% (26,069/70,665) | 7.9% (3,697/46,997) | 10.6% (7,604/71,783) | 0.1% (102/71,783) | 78.4% (56,208/71,681) |
| fission 18f9e3d | 40.0% (28,706/71,783) | 35.1% (24,809/70,665) | 7.6% (3,591/46,997) | 9.0% (6,481/71,783) | 0.0% (0/71,783) | 85.0% (60,985/71,783) |
| ghidra 12.2 | 24.7% (17,745/71,783) | 21.0% (14,870/70,665) | 5.0% (2,337/46,997) | 7.0% (5,033/71,783) | 30.9% (22,191/71,783) | 78.2% (38,777/49,592) |

distance (mean · median · at-0 / measured)

| tool | ged | type_match | byte_match |
|---|---|---|---|
| borer | 21.7 · med 5 · 28,766/70,665 | 3.4 · med 2 · 3,257/46,997 | 48.3 · med 12 · 8,516/43,101 |
| kuna | 22.1 · med 5 · 26,428/70,665 | 3.1 · med 2 · 4,276/46,997 | 87.9 · med 19 · 8,205/55,880 |
| angr | 21.2 · med 5 · 26,069/70,567 | 3.2 · med 2 · 3,697/46,949 | 62.3 · med 22 · 7,604/56,208 |
| fission | 29.1 · med 6 · 24,809/70,611 | 3.5 · med 2 · 3,591/46,997 | 69.5 · med 26 · 6,481/60,985 |
| ghidra | 26.0 · med 7 · 14,870/48,874 | 3.5 · med 2 · 2,337/33,686 | 99.9 · med 23 · 5,033/38,777 |

Project bootstrap (10,000 resamples of the 20 projects), Union:

| tool | Union | 95% interval | ranked first |
|---|---:|---:|---:|
| borer | 45.2% | 40.4-49.8% | 99.98% |
| kuna | 42.8% | 37.6-47.8% | 0.00% |
| angr | 42.7% | 38.3-46.9% | 0.02% |
| fission | 40.0% | 35.1-44.8% | 0.00% |
| ghidra | 24.7% | 20.8-28.9% | 0.00% |

borer - Kuna: +2.39 pp (95% +1.21 to +3.65).

Cost (batch wall time per function, per binary)
| tool | median s/fn | mean s/fn | functions |
|---|---:|---:|---:|
| angr | 0.651 | 1.147 | 71,681 |
| borer | 0.039 | 0.163 | 71,783 |
| fission | 0.096 | 0.268 | 71,783 |
| ghidra | 0.860 | 3.671 | 49,592 |
| kuna | 0.190 | 1.520 | 71,783 |

Caveats specific to this table:

* **borer**: release `borer-v0.3.1-x86_64-unknown-linux-gnu.tar.gz`, sha256
  `d3b7f894edf9703e6e3c128332a62c9acdb49d6e2ce9ceb2e1aab0fec3845d53`, run on CI only (run 45066341615).
  DecBench has no borer driver; `pipeline/borer_raw.py` (`borer export --c`, split per function) is a
  FreshBench-side backend. Everything after the decompile is DecBench's path.
* **Types**: borer and Fission are scored on DecBench's legacy text path (`parse_c_variables`); Ghidra,
  angr and Kuna on address correspondence. type_match is not comparable across those two groups.
* **byte_match on Clang builds**: DecBench's fix-up loop parses GCC diagnostics only, so on Clang builds
  the loop runs on GCC diagnostics and the compared object is built with Clang 23.1.6
  (`pipeline/score_bytematch.py`). DecBench itself recompiles every ELF with `gcc`.

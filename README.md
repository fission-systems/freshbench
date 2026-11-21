# FreshBench (working name)

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

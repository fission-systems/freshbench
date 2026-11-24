# FreshBench

FreshBench is a decompiler benchmark built on fresh, license-clean C
programs that no decompiler has been tuned on. It does two jobs: it compares
decompilers on the same functions, and it shows each decompiler where it can
improve. It has its own ground truth and its own metrics, written in Rust
(`crates/freshbench`).

**Status (2026-11-24):** v2 is in progress. The scorer, the CFG builder, GED,
`gaps` and manifests are done and tested. Ground-truth extraction, the tool
adapters and the M1 corpus are being built. The v1 release (DecBench-based)
is preserved at tag [`v1`](../../tree/v1) and in
[`sjkim1127/freshbench`](https://huggingface.co/datasets/sjkim1127/freshbench).
v1 numbers and v2 numbers are not comparable.

## Prior work

FreshBench's metric families follow [DecBench](https://github.com/Noelo-Lab/decbench)
(Noelo-Lab): control-flow graph edit distance against the source, type recovery
against DWARF, recompilation byte match, and a Union of perfect scores.
FreshBench v1 was built on DecBench and its rust-joern CFG parser. v2 uses no
DecBench or rust-joern code. Its CFG definition, GED computation, and
ground-truth path are its own, documented below and in the source. Its scores
are therefore not DecBench scores. See `NOTICE`.

## What is measured, and what is not

**Measured: how close the output is to the source, and a static agreement
signal against the binary.**

| metric | compares | perfect |
|---|---|---|
| GED | the output's CFG vs the source function's CFG, both built by FreshBench's one CFG builder (`cfg.rs`) | 0 |
| size bands | every table split by source CFG nodes: <6, 6-20, 21-50, 51-100, >100 | |
| normalized GED | GED / (source nodes + edges) | 0 |
| call sites | direct call instructions in the unstripped binary vs calls printed (`callsites.rs`) | nothing missing, nothing extra |
| shape tags | over/under-noded, extra goto/return/ternary/calls, lost `&&`/`||`, lost switch (`taxonomy.rs`) | |

Planned: type recovery against DWARF, with coverage and accuracy reported
apart, and recompilation.

**Not measured: correctness.** GED sees only graph shape. A function that
prints the wrong constant, the wrong argument or an invented call can score
perfect, and recovering missing code can make the score worse. Call-site
agreement checks that each call is printed once, not that its arguments are
right. Use FreshBench to compare decompilers on the same rows and to find
regressions. A better score is not evidence that a change is correct, and a
worse one is not evidence that it is wrong.

## How GED is computed

* **One CFG builder for both sides.** Source and output are parsed with
  tree-sitter-c and built by the same code. Two texts with the same control
  flow therefore get the same graph, whatever syntax they print it with.
  * `&&`, `||` and `?:` are control flow, as in the machine code, so
    `if (a && b)` and `if (a) if (b)` match.
  * Literal conditions are folded (`while (1)`, `do {} while (0)`).
  * `goto`, `break`, `continue` and `return;` are edges, not blocks.
  * Calls to no-return functions end at EXIT.
  * Empty blocks are removed, chains are merged, and unreachable code is
    dropped.
* **Edit costs.** Inserting or deleting a node or an edge costs 1.
  Substituting a node costs 0 for the same label (ENTRY, EXIT, block) and 1
  otherwise.
* **Search.** The lower bound is the size difference. The upper bound comes
  from a Hungarian node assignment plus deterministic local search. A
  branch-and-bound search then tightens it within a fixed work count, never a
  time limit. Each row records `exact` and `lower_bound`. A value of 0 is
  always exact.
* **Deterministic.** Identical inputs give byte-identical rows on any machine.
  `freshbench determinism` checks that no (function, output hash) pair has two
  scores.

Measured on 200 source TUs of the v1 corpus (Fission's outputs, 2,446 rows):
two runs were byte-identical; 30 s wall / 87 s CPU on 14 cores; 13% of rows
have an upper bound only.

## Improvement targets: `freshbench gaps`

```sh
cargo build --release
target/release/freshbench rescore <dataset> --tool fission --out rows/fission.jsonl
target/release/freshbench gaps <dir-with-rows> --tool fission --json gaps.json
```

For every function, the tool is compared with every other tool on the same row:

* **reachable**: another decompiler scored GED 0 from the same stripped
  binary and this one did not. This is an existence proof that the source's
  shape is recoverable.
* **gap**: this tool's GED minus the best other tool's GED.
* **no score**: this tool has nothing scorable where another tool scored.

Results are grouped by size band, outcome and shape tag. Each group lists
example rows, worst first. These are similarity gaps, not correctness
defects. Open the output before acting on one.

Other commands: `bands`, `determinism`, `score` (one function), `cfg` (print
the graph FreshBench builds), `callsites`, `manifest build|verify|summary`.

## Corpus

| milestone | languages | architectures | compilers | optimization | status |
|---|---|---|---|---|---|
| v1 | C | x86-64 | GCC 16.2.0, Clang 23.1.6 | O0 O1 O2 O3 Os O2-noinline | released (tag `v1`, DecBench-based) |
| **M1** | C | x86-64, i686, AArch64, ARMv7 (Thumb-2, hard float) | GCC 16.2.0, Clang 23.1.6 | same six | building |
| M2 | C++, C | + PE/PDB | + MSVC, MinGW-w64 | | planned |
| M3 | Rust, Go | | rustc, gc | | planned; no C source CFG, so binary-grounded signals only |

Rules:

* **Fresh, licensed sources.** Only GitHub projects with an OSI license are
  used. Each project's license file, SPDX id and commit ship with the dataset.
  GPL binaries ship with their preprocessed sources and the upstream commit.
  No malware.
* **One exact toolchain per binary.** The build image is pinned by digest.
  The cross GCC is built from the same GCC 16.2.0 release as the native one,
  and Clang cross-compiles with the same 23.1.6 (`build/Dockerfile.cross`).
  Each manifest entry records the compiler version line, target and flags.
* **Stripped input only.** Decompilers see stripped copies. Names, addresses
  and types come from the unstripped original, for grading only.
* **Static only.** No corpus binary is ever executed.
* **Same functions across the matrix.** Selection order is
  sha256(`project/source/name`), which does not depend on the architecture or
  compiler.

Storage: the harness and per-milestone manifests (path, size, sha256,
provenance) are in this repository. Corpus files and results are on
Hugging Face at a pinned revision (`sjkim1127/freshbench-v2` from M1 on).
Git LFS is not used.

## Layout

| path | what |
|---|---|
| `crates/freshbench/` | the scorer and CLI (Rust): `csyntax`, `cfg`, `ged`, `score`, `dataset`, `taxonomy`, `headroom`, `bands`, `callsites`, `manifest`, `rows` |
| `build/` | build images (`Dockerfile` = v1 x86-64, `Dockerfile.cross` = v2 cross toolchains), compiler wrapper `fbcc.py`, `build_project.py` |
| `ci/`, `.github/workflows/` | pins, corpus qualification per architecture, image builds, tests |
| `selection/` | candidate enumeration and corpus lists (`corpus100.json`, `m1/`) |
| `pipeline/` | v1 pipeline (DecBench-based); replaced by Rust adapters in v2 |

Harness: Apache-2.0 (`LICENSE`). Corpus files keep their projects' licenses.

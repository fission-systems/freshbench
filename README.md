# FreshBench

FreshBench evaluates decompilers on fresh, license-clean C programs that no
decompiler has been tuned on, and tells each decompiler where it can improve.
It keeps DecBench's metrics and aggregation, so its headline numbers are
DecBench numbers. Around them it adds size-banded tables, deterministic
scoring, static call-site agreement, and a per-function *headroom* report.

## What is measured, and what is not

**Measured: similarity to the source.** For every selected function, each
decompiler's output is compared with the original program:

| metric | compares | perfect |
|---|---|---|
| `ged` (DecBench) | control-flow graph of the output vs the source CFG (rust-joern), graph edit distance | 0 |
| `type_match` (DecBench) | recovered variable types vs DWARF | all matched variables right |
| `byte_match` (DecBench) | recompiled output vs the original code bytes | identical |
| Union (DecBench) | perfect on at least one of the three | |
| size bands (FreshBench) | every table split by source CFG nodes: <6, 6-20, 21-50, 51-100, >100 | |
| normalized GED (FreshBench) | GED / (source nodes + edges) | 0 |
| call sites (FreshBench) | direct call instructions in the unstripped binary vs calls printed | missing = excess = 0 |

**Not measured: correctness.** None of these metrics checks that the output
means what the binary does. GED sees only graph shape: a function that prints
the wrong constant, the wrong argument or an invented call can score perfect,
and recovering missing code can make the score worse. type_match covers only
the variables it can match. Call-site agreement checks that each call is
printed once, not that its arguments are right. Use FreshBench to compare
decompilers on the same rows and to find regressions. Do not read a better
score as proof that a change is correct, or a worse one as proof that it is
wrong.

## Corpus

| milestone | languages | architectures | compilers | optimization | status |
|---|---|---|---|---|---|
| v1 (2026-11-24) | C | x86-64 | GCC 16.2.0, Clang 23.1.6 | O0 O1 O2 O3 Os O2-noinline | published, see [README-v1.md](README-v1.md) |
| **M1** | C | x86-64, i686, AArch64, ARMv7 (Thumb-2, hard float) | GCC 16.2.0, Clang 23.1.6 | same six | in progress |
| M2 | C++, C | + PE/PDB | + MSVC, MinGW-w64 | | planned |
| M3 | Rust, Go | | rustc, gc | | planned; no source CFG, so binary-grounded signals only |

Rules every milestone keeps:

* **Fresh, licensed sources.** GitHub projects with an OSI license, selected
  in sha256 order of their name and filtered against DecBench's projects by
  CFG fingerprint (`selection/`, `overlap/`). Each project's license file,
  SPDX id and commit ship with the dataset; GPL binaries ship with their
  preprocessed sources and the upstream commit. No malware, ever.
* **One exact toolchain per binary.** The build image is pinned by digest. The
  cross GCC is built from the same GCC 16.2.0 release as the native one
  (`build/Dockerfile.cross`), and Clang cross-compiles with the same
  23.1.6. Every binary's manifest entry records the compiler version line,
  target triplet and flags. A substituted compiler would make a harness
  failure look like a decompiler failure.
* **Stripped input only.** Decompilers see stripped copies. Names, addresses
  and types come from the unstripped original, for grading only.
* **Static only.** No corpus binary is ever executed.
* **Same functions across the matrix.** Up to 300 functions are selected per
  build, in sha256 order of `project/source/name`. That order does not
  depend on the architecture or compiler, so where a function is eligible
  everywhere it is the same row on every architecture.

Storage: the harness, selection lists and manifests (path, size, sha256 and
provenance of every corpus file) are in this repository. The corpus files
and results are on Hugging Face at a pinned revision:
[`sjkim1127/freshbench`](https://huggingface.co/datasets/sjkim1127/freshbench)
for v1 and `sjkim1127/freshbench-v2` from M1 on. Git LFS is not used,
because corpus revisions replace whole trees and LFS storage is not freed
when files are deleted.

## Scoring changes over DecBench

DecBench's own numbers are reported unchanged (`pipeline/decbench_site.py`
calls `decbench.rendering.aggregate`). FreshBench adds the following.

* **Deterministic scores** (`freshbench/scorecache.py`). Every output is
  hashed before scoring, and a score is keyed by (scorer pin, metric, ground
  truth, output hash), so identical text always gets the identical score.
  `freshbench determinism ROOT` lists any (ground truth, output) pair scored
  two ways. Every scoring job also rescores the GED of a fixed, name-chosen
  1% of its binaries (at least one) in a new process. Any change is recorded
  in `run.json` (`ged_rescore`), and the row is marked `ged_unstable`, which
  `determinism` also reports. DecBench's GED has been seen to score the same
  text 0 in one run and 11 in another. The content-addressed cache
  (`ScoreCache`) is in the package, but `run_tool.py` does not use it yet.
* **Answer-key hygiene.** A row is a DWARF subprogram that has one address in
  its binary and one preprocessed text for its source file, and whose
  low_pc lies in an executable section with bytes. Names shared by several
  definitions are excluded and counted, not guessed (`pipeline/prepare.py`).
* **Type coverage apart from accuracy.** At -O2 most variables live in
  registers that the ground truth cannot locate, so type_match is bounded by
  recall. `freshbench types` reports coverage (DWARF variables matched) and
  accuracy (matched variables typed right) apart, so a tool that scores
  better by matching fewer variables shows as lower coverage.
* **Size bands.** GED grows with function size, and no tool scores perfect
  above about 100 nodes. Totals are dominated by a few large functions, so
  rankings are also given per band, with normalized GED.
* **Call-site agreement** (`freshbench/callsites.py`). This is a static,
  tool-neutral signal reported beside Union and never folded into it.

## Improvement targets: `freshbench gaps`

```sh
pip install -e .
freshbench gaps <dataset-dir> --tool fission --json gaps.json
```

For every row it compares the tool with every other tool on the same row:

* **reachable**: another decompiler scored GED 0 from the same stripped
  binary and this one did not. This is an existence proof that the source's
  shape can be recovered.
* **gap**: this tool's GED minus the best other tool's GED.
* **no output**: this tool printed nothing where another tool scored.

Rows are grouped by size band, by outcome (`abstain`, `no_output`,
`unscored`, `perfect`, `imperfect`) and by shape tags computed from the
output and the source function with one lexical C tokenizer
(`freshbench/ctext.py`, `freshbench/taxonomy.py`). The tags are
`over_noded`, `under_noded`, `extra_goto`, `extra_return`, `lost_logical`,
`extra_ternary`, `extra_calls`, `missing_calls` and `lost_switch`. Each
group lists example rows, worst first. Gaps are similarity gaps, not
correctness defects. Open the output before acting on one.

Other commands:

```sh
freshbench bands <dataset-dir>              # perfect rate + median normalized GED per tool and band
freshbench types <dataset-dir>              # type coverage vs accuracy per tool
freshbench determinism <dataset-dir>        # must print 0 pairs
freshbench manifest verify <dataset-dir> corpus/<milestone>/manifest.jsonl
freshbench callsites --objdump-text <llvm-objdump -d --no-show-raw-insn of the UNSTRIPPED binary> \
    --rows rows.jsonl --records records.jsonl.gz
```

## Reproducing a run

1. **Corpus.** Download the dataset revision named in
   `corpus/<milestone>/README.md`, then run `freshbench manifest verify`.
   To rebuild it instead, dispatch `qualify-arch.yml` with the milestone's
   candidate list. Every build runs in `BUILD_IMAGE_CROSS` from
   `ci/pins.env` with `--network none`.
2. **Decompile and score.** Dispatch `decompile.yml`. Every decompiler
   (Ghidra, angr, Kuna, Fission, borer) runs through DecBench's raw backend,
   pinned in `ci/pins.env` (DecBench, rust-joern, tool versions, images).
   To measure a Fission branch, pass `fission_ref=refs/heads/<branch>` and
   `tools=["fission"]`.
3. **Aggregate.** `pipeline/decbench_site.py` produces DecBench's tables.
   `freshbench bands` and `freshbench gaps` produce FreshBench's.

Changing any pin invalidates earlier results. Compare runs only under one
set of pins. Compare output hashes first, then scores.

## Layout

| path | what |
|---|---|
| `freshbench/` | v2 package: `bands`, `ctext`, `taxonomy`, `headroom`, `callsites`, `scorecache`, `manifest`, `rows`, `cli` (standard library only) |
| `tests/` | pytest suite (`python -m pytest`); CI runs it on every push |
| `build/` | build images (`Dockerfile` v1 x86-64, `Dockerfile.cross` v2), compiler wrapper `fbcc.py`, `build_project.py` |
| `ci/` | `pins.env`, `qualify.sh` (v1), `qualify_arch.sh` (v2), scorer/tool setup |
| `pipeline/` | prepare, run_tool (DecBench drivers), scoring, aggregation, dataset assembly |
| `selection/` | candidate enumeration, exclusions, corpus lists (`corpus100.json`, `m1/`) |
| `corpus/` | per-milestone manifests (hashes + provenance) |

Harness code: Apache-2.0 (`LICENSE`). Corpus files keep their projects'
licenses, recorded per project in the dataset.

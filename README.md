# FreshBench

FreshBench is a decompiler benchmark built on recent, license-clean C
programs chosen to be outside the corpora decompilers are usually tuned on
(DecBench's functions are excluded by fingerprint; this lowers the chance
that a tool has seen them, it does not prove it). It does two jobs: it compares
decompilers on the same functions, and it shows each decompiler where it can
improve. It has its own ground truth and its own metrics, written in Rust
(`crates/freshbench`).

**Status (2026-11-25):** v2 is in progress. The scorer, the CFG builders,
GED, type and recompile metrics, `gaps` and manifests are done and tested.
The M1 corpus is published ([`corpus/m1`](corpus/m1/README.md)); tool
results on it are being produced. The v1 release (DecBench-based)
is preserved at tag [`v1`](../../tree/v1) and in
[`sjkim1127/freshbench`](https://huggingface.co/datasets/sjkim1127/freshbench).
v1 numbers and v2 numbers are not comparable.

## M1 leaderboard

The GitHub Pages workflow ([pages](../../actions/workflows/pages.yml)) builds
the public leaderboard from completed `evaluate` runs. Supply their run IDs
as a JSON array. It downloads only the pinned M1 target manifests (not the
13.56 GB corpus), verifies their SHA-256 digests and checks that at least two
registered non-LLM tools have a row for every M1 target. Explicit failed or
unsupported rows count; absent result rows do not pass the publication gate.

Leave `deploy` false to inspect the generated `leaderboard-preview` artifact
first. Enable GitHub Pages with GitHub Actions as its source, then run with
`deploy` true to publish. The intended site is
https://fission-systems.github.io/freshbench/. The all-row and renamed-clone
deduplicated views link to each other and expose JSON summaries, metrics,
and per-tool improvement targets. Function boundaries and timing are shown
when their artifacts are supplied. GED is a structural metric, not a
correctness guarantee; rows with unknown source graph size are explicitly
reported outside the distance aggregate.

## Prior work

FreshBench's metric families follow [DecBench](https://github.com/Noelo-Lab/decbench)
(Noelo-Lab): control-flow graph edit distance against the source, type recovery
against DWARF, recompilation byte match, and a Union of perfect scores.
FreshBench v1 was built on DecBench and its rust-joern CFG parser. v2 uses no
DecBench code. Its own CFG definition, GED computation, type and
recompilation scoring, and ground-truth path are documented below and in the
source. For the second CFG family it vendors
[rust-joern](https://github.com/Noelo-Lab/rust-joern) (Apache-2.0,
`crates/rust-joern/`, upstream main plus the open pull requests #21-#68; see
`VENDORED.md`), but scores its graphs with FreshBench's GED. Its scores are
therefore not DecBench scores. See `NOTICE`. DecBench's maintainer agreed
to this reuse of its metric design on 2026-11-25, with one request we keep:
be honest about duplication. Row counts overstate how many different
functions a corpus holds; see
[how many functions M1 really is](corpus/m1/README.md#how-many-functions-this-really-is).
The scoring defects DecBench found over the years are tracked against this
implementation, each with its status and a regression test, in
[`docs/decbench-compatibility-audit.md`](docs/decbench-compatibility-audit.md).

## What is measured, and what is not

**Measured: how close the output is to the source, and a static agreement
signal against the binary.**

**What is ranked.** The main metrics are the GED distance from the source
(mean, median and rows at 0 over every row; a row without a score is charged
the whole source graph) and function boundaries (start F1 and exact ends of
the functions a tool finds by itself, with and without unwind tables). Perfect
rates, Joern GED, types and recompilation (byte match) are auxiliary: shown,
not ranked. Speed is reported next to them (CPU time per function, peak
memory).

**Who is published.** Only decompilers in the tool registry
(`ci/tools.json`) whose upstream repository had a commit within the last
365 days when the page is built; an unmaintained project's numbers stay in
the raw results but are not published. LLM-based decompilers are not ranked:
the corpus is public, so a model may have seen it. They would need a separate
track with its own contamination controls.

**Line mappings are required.** Every decompiled function must come with the
tool's own line mappings (which instructions each output line came from) and,
for types, its variables with ABI argument positions and stack slots, as in
DecBench's native line-mapping contract (Noelo-Lab/decbench#129). Addresses are
checked against the function's instructions; an output with no valid line
mapping is not scored. Types are matched to DWARF through this evidence only,
never by reading the C text. GED still reads the output's C, since the
structure is what it measures.

**No fixups.** Every metric reads the decompiler's output exactly as it was
printed. FreshBench does not rewrite keywords, define pseudo-types, inject
declarations the compiler asks for, or repair code before compiling or
parsing it. Such fixups measure the fixer, and a fixer written around one
decompiler's dialect favours that decompiler. Output that needs a type or
declaration it did not print fails to compile, and that counts against the
decompiler that printed it. The only uniform inputs are the compiler flags
(`-w -fpermissive -std=gnu17 -fno-builtin` plus the build's own) and, for
choosing the scored function, the rule "the function with the source's name,
else the only function defined in the output" (prototypes are not
definitions), applied the same way for both CFG families.

| metric | compares | perfect |
|---|---|---|
| GED | the output's CFG vs the source function's CFG, both built by FreshBench's one CFG builder (`cfg.rs`) | 0 |
| Joern GED | the same comparison on Joern's basic-block CFG (vendored rust-joern, `joern.rs`), scored with the same GED engine | 0 |
| types | return and parameter type classes (`int<N>`, `float<N>`, bool, ptr, array, struct, void) vs DWARF (`types.rs`, `dwarf.rs`); locals as coverage only | arity, every parameter and the return right |
| recompile | the output exactly as printed (no prelude, no injected declarations, no repair loop), built by the binary's own compiler and flags plus one fixed permissive flag set; its instructions vs the original's with every constant, displacement and register kept, where only an operand the object file has a relocation for may differ (`recompile.rs`) | instruction-identical |
| Union | any of GED, Joern GED, types or recompile perfect (one is enough: a right signature with a wrong body counts) | |
| Intersection, both CFGs, strict | every metric that applies perfect; GED 0 on both graph families; GED 0 from output that parsed without error recovery | |
| with control flow | the perfect rate over rows whose source function has a branch or a loop. A source without control flow is `entry -> exit` and matches any output without control flow, an empty body included, so those perfects carry no information about structure and are also reported as their own `straight-line` band | |
| size bands | every table split by source CFG nodes: straight-line (≤2), 3-5, 6-20, 21-50, 51-100, >100 | |
| normalized GED | GED / (source nodes + edges) | 0 |
| call sites | direct call instructions in the unstripped binary vs calls printed (`callsites.rs`) | nothing missing, nothing extra |
| shape tags | over/under-noded, extra goto/return/ternary/calls, lost `&&`/`||`, lost switch (`taxonomy.rs`) | |
| function boundaries | a separate track (`boundaries.rs`): the tool gets **no** addresses and lists the functions it finds in the stripped binary; starts and ends vs the unstripped binary's `STT_FUNC` symbols and DWARF ranges. Two variants: `as-built`, and `no-unwind` with `.eh_frame`/`.eh_frame_hdr`/`.ARM.exidx`/`.ARM.extab` zeroed, because `.eh_frame` survives stripping and gives every function's exact range away on x86-64 and AArch64 Linux. Project functions (with DWARF) are reported apart from runtime code, and identical binaries are also counted once | start precision/recall/F1; exact and padded ends |

Recompilation runs only inside the build image, which has the compilers.

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

* **Score cache.** A score is keyed by (scorer, metrics revision, budget,
  function, source definition fingerprint and DWARF signature, output hash).
  The metrics revision changes whenever a CFG builder or the vendored
  rust-joern changes, so a cache never returns a score from an older
  definition. The same source function and the same
  output text are scored once, across builds, tools and runs.
  `rescore --cache FILE` keeps the cache between runs.

Measured on the whole v1 corpus (Fission's outputs, 354,167 rows, 14-core Mac,
budget 200k, other work on the machine):
* **Cold cache:** 3,151 s CPU (8.9 ms per row), 380 s wall. 16% of rows hit
  the cache within the run (identical output in another build).
* **Warm cache:** 247 s CPU (0.7 ms per row), 40 s wall. The rows are
  byte-identical to the cold run.
* **Upper bound only:** 25% of scored rows. Raising the budget to 2M on a
  2,446-row sample cut that from 15% to 13%, changed the GED total by 0.7%,
  changed no perfect row, and cost 5x the CPU.

For reference, DecBench's scorer in the full-dataset sweep used 4.5 ms per
row. In both sweeps decompiling costs far more than scoring (Fission: 4,554 s
CPU for 96,103 functions). With a warm cache a before/after comparison
rescores only the rows whose output changed.

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

Other commands: `sweep` (below), `boundaries`, `duplicates`, `bands`, `determinism`, `score` (one function), `cfg` (print
the graph FreshBench builds), `callsites`, `manifest build|verify|summary`.

## Sweep (one Fission build over the corpus)

The quality loop for a decompiler under development (`sweep.rs`):

```bash
freshbench sweep run data/m1 --cli target/release/fission_cli --repo ~/Fission --label main
freshbench sweep report runs/main          # scores, failures, timing
freshbench sweep diff runs/main runs/fix   # output hashes first, then scores on changed rows
```

* `run` writes `runs/<label>/`: `meta.json` (CLI sha256, git commit, scorer id,
  function-set id, every session's wall time), one checkpoint per binary
  under `bin/`, and `rows.jsonl`. An interrupted run continues from its
  checkpoints; a resume with another CLI or function set is refused. After
  the scorer changed, the same command keeps the outputs and scores them again.
  `--builds`, `--only` narrow the run; `--jobs 0` (default) runs half the
  cores' worth of binaries at once.
* A call that crashes or times out is bisected until the failing function is
  alone; the other functions of the binary keep their output.
* The scorer is the `freshbench` binary: its id is a hash of the scoring
  sources, written into every row, and `diff` refuses runs scored by
  different scorers. Scores are cached by content under `runs/score-cache.jsonl`.
* `diff` compares the sha256 of each output first. Identical outputs are not
  compared on score (and are flagged if their scores differ). On changed rows
  it reports perfects gained and lost, the same on functions with control
  flow, Joern and type perfects, the distance delta and the rows that moved
  most.
* `report` gives the perfect count next to the total distance, split by
  architecture, compiler, level and size band, and what the run cost: wall
  time, CPU time of every `fission_cli` process, CPU per function and peak
  memory. `diff` compares CPU time between the two runs.

## Speed

Every tool run is measured in Rust (`adapters::run_measured`, `wait4`): wall
time, user + system CPU time of the tool's whole process tree, and the peak
resident memory of its largest process. `decompile` writes one line per
binary to `results/<tool>/<build>/<project>/timing.jsonl`; boundary rows and
sweep runs carry the same figures.

```bash
freshbench speed results/ boundaries/ runs/main/timing.jsonl
```

prints, per track and tool, CPU milliseconds per function, total CPU and wall
time, median / p90 / max CPU seconds per binary and median / max memory. CPU
time is the figure to compare between tools; wall time depends on how many
binaries ran at once and on the machine.

Measured on one M1 project (12 binaries, 3,600 functions, 14 cores): 75 s to
decompile at 7 binaries at once, 53 s to score cold and about 1 s from the
cache; a resumed run decompiles only what has no checkpoint.

## Corpus

| milestone | languages | architectures | compilers | optimization | status |
|---|---|---|---|---|---|
| v1 | C | x86-64 | GCC 16.2.0, Clang 23.1.6 | O0 O1 O2 O3 Os O2-noinline | released (tag `v1`, DecBench-based) |
| **M1** | C | x86-64, i686, AArch64, ARMv7 (Thumb-2, hard float) | GCC 16.2.0, Clang 23.1.6 | same six | corpus published ([`corpus/m1`](corpus/m1/README.md)); tool results pending |
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
| `crates/freshbench/` | the scorer and CLI (Rust): `csyntax`, `cfg`, `ged`, `joern`, `types`, `dwarf`, `recompile`, `score`, `dataset`, `prepare`, `adapters`, `report`, `taxonomy`, `headroom`, `bands`, `callsites`, `manifest`, `rows` |
| `crates/rust-joern/` | vendored rust-joern (Apache-2.0), see its `VENDORED.md` |
| `build/` | build images (`Dockerfile` = v1 x86-64, `Dockerfile.cross` = v2 cross toolchains), compiler wrapper `fbcc.py`, `build_project.py` |
| `ci/`, `.github/workflows/` | pins, corpus qualification per architecture, image builds, tests |
| `selection/` | candidate enumeration and corpus lists (`corpus100.json`, `m1/`) |
| `pipeline/` | v1 pipeline (DecBench-based); replaced by Rust adapters in v2 |

Harness: Apache-2.0 (`LICENSE`). Corpus files keep their projects' licenses.

# DecBench defects, checked against FreshBench

FreshBench shares no scoring code with DecBench, but most defects DecBench
found over the years are defects in how a decompiler benchmark can go wrong,
and a new implementation can make them again. This file lists the DecBench
issues and pull requests that have a counterpart here, what FreshBench does
about each, and the test that keeps it that way.

Status words:

* **fixed**: FreshBench had or could have had the defect; it is handled and a
  test fails if it comes back.
* **by design**: the defect cannot occur because of how FreshBench is built; a
  test pins the property where one is cheap.
* **partial**: handled in part; what is missing is stated.
* **open**: not handled.

"Measured" means the defect was reproduced in FreshBench before the fix.
Tests named `decbench_NNN_*` are in `crates/freshbench/tests/decbench_regressions.rs`.

| DecBench | defect | status | FreshBench |
|---|---|---|---|
| #5 | a timed-out tool leaves child processes | fixed (measured) | adapters run in their own process group; a timeout kills the group. `decbench_005_*` |
| #6 | a degenerate source CFG rewards truncated output | partial (measured) | A source without control flow is `entry -> exit`, and an empty body matches it: on one M1 project 18.6% of rows are such sources and they hold 50-70% of each tool's perfect scores. They are kept, but reported in their own `straight-line` band and left out of "perfect on functions with control flow". Not done: M1's selection still includes them, and the share over all of M1 is not measured yet. `decbench_006_*`, `report::tests::straight_line_sources_are_reported_apart` |
| #8 | tools scored over different denominators | fixed (measured) | every tool is counted on every row of the table; a missing result is not perfect. With `report --corpus` the table is the corpus's selected functions, so a row no tool has a result for counts too. `decbench_008_*` |
| #37 | PE/PDB metrics | open | M2. |
| #38 | results published with silent data loss | partial | `manifest verify` (every file, size and sha256); `load_table` refuses mixed scorers, conflicting duplicate rows and stale recompile side files; pages take their metadata from each run's own provenance. Not done: publishing is not atomic. |
| #43 | LLM contamination and bias | partial | sources are recent, DecBench's functions are excluded by token fingerprint, and `duplicates --against` counts the remaining overlap on every architecture (7-8 exact, 282-312 renamed copies, all under 50 tokens). This does not prove that no tool was tuned or trained on a corpus function. |
| #50 | one malformed output breaks a run | fixed | a scorer panic costs one row (`scorer_panic`); a failed whole-binary call is retried per function. `adapters::tests::script_contract_and_per_function_retry` |
| #52 | GED cutoff too aggressive | fixed | the search is bounded by work, not time; every score says whether it is exact and carries a lower bound; `gaps` separates proven from inconclusive gaps. |
| #57 | large isomorphic CFGs not scored 0 | fixed (measured) | a 400-node isomorphic pair scored above 0 under a small budget. `ged::isomorphic` (colour refinement, then search, own step cap) runs before the GED search; when it runs out of steps the score's method says `isomorphism unknown`. `decbench_057_*`, `ged::tests::*isomorph*` |
| #58 | code in several executable sections discarded | by design, no fixture | truth and boundaries take any executable section that has bytes in the file; a tool that drops such code loses those functions. |
| #61 | ARM encoding ignored in byte match | partial | both sides are disassembled by address range, so a literal pool is on both or neither; tested with real ARMv7 objects (`recompile::tests::a_wrong_constant_is_never_exact_on_any_isa`). Not done: no Thumb-2 fixture. |
| #98 | limits of type scoring | open | classes only (int width, float width, bool, pointer, array, struct, void); no typedef names, no struct layout, no signedness. |
| #100 | function bytes read from the wrong section | by design | the original is cut by `[low_pc, high_pc)`, the recompiled object by its symbol's own section and range. |
| #101 | another binary's `main` used as the source | by design | a function is (project, source file, name) and is selected in one binary per build; its source text is the translation unit DWARF names. |
| #104 | LP64 sizes applied to ARM32 | fixed | `DataModel::for_arch`: ILP32 on i686 and ARMv7; `long double` 12 / 8 bytes there. The data model is part of the score-cache key. `decbench_104_*`, `dataset::tests::cache_does_not_cross_data_models` |
| #105 | `while (1)` and `for (;;)` differ | by design | one CFG builder for both sides. `decbench_105_*` |
| #107 | foreign-arch binaries stripped wrongly | by design | the strip tool is recorded per binary, and `prepare::verify_strip` fails the build unless the stripped copy kept every code byte and lost its symbol table and debug sections. |
| #109 | stale source CFGs reused | by design | no source CFG is stored; it is built from the manifest-pinned source at scoring time, and its hash is in the score-cache key. |
| #112 | `restrict` / `_Atomic` hide the type | by design | read through on the output side (`decbench_112_*`) and in DWARF (`DW_TAG_restrict_type`, `DW_TAG_atomic_type`); no DWARF fixture. |
| #113 | an inlined callee's variables scored | by design, no fixture | only the function's own `DW_TAG_variable`s, lexical blocks included, inlined subroutines excluded. |
| #114 | per-decompiler fixups | by design | no fixups at all: output is compiled and parsed as printed. `recompile::tests::recompile_roundtrip_when_llvm_present` |
| #119 | same-named functions overwrite each other | partial | a name with two definitions in a translation unit, or two addresses in a binary, is not selected, so nothing is merged; but such functions are lost instead of told apart. Planned: identity by (binary sha256, entry address, DIE), needed before C++. `decbench_119_*` |
| #120 | DWARF reference forms | fixed (measured) | with GCC 16.2.0 `-flto`, every type of a function read as void. References into other units (`DW_FORM_ref_addr`) are followed, and a type that cannot be read is `unknown`, never `void`. `decbench_120_*` with a GCC LTO fixture. Not done: LTO units are still not selectable (their source file is `<artificial>`). |
| #122 | DWARF 5 indexed locations | partial | location lists are read through gimli (`.debug_loc` and `.debug_loclists`, `loclistx` included) and an empty list is not a location. No DWARF 5 `loclistx` fixture. |
| #127 | scores depend on compiler version, stale cache | partial | the compiler is pinned by image digest and checked against `DW_AT_producer`; the score cache is keyed by a hash of the scoring sources. Not done: recompile results are not tied to the compiler binary's hash. |

## Found here, not in DecBench's list

| defect | status | FreshBench |
|---|---|---|
| recompile normalization erased constants (`#` cut as a comment on ARM/AArch64, every hex token made `ADDR`) | fixed (measured) | instructions are compared whole; only a relocated operand may differ. `recompile::tests::*` |
| the Joern score skipped outputs that name the function themselves and declare callees (prototypes counted as functions) | fixed (measured: Fission 194 -> 775, borer 455 -> 1,088 Joern perfects on one project) | `joern::tests::output_function_beside_prototypes` |
| score cache shared between data models | fixed | `dataset::tests::cache_does_not_cross_data_models` |
| `.eh_frame` gives function boundaries away | handled | boundary track has a `no-unwind` variant. `boundaries::tests::unwind_tables_are_removed_when_clang_present` |

## Rules these come from

1. Ground truth first: do not score against a wrong answer.
2. No silent failure: a missing, conflicting or unparsable result is counted and named.
3. No false perfect: a failed measurement is not a perfect score.
4. No cross-tool bias: nothing is rewritten for one decompiler's dialect.
5. Full provenance: every result says what produced it.
6. Regressions are permanent: a scoring defect becomes a test.

# AGENTS.md

FreshBench is an independent decompiler benchmarking and regression-analysis framework. It measures how close output is to source, not whether it is correct.
It is not a DecBench clone: compatibility never justifies reproducing a known defect, and it is not a Fission benchmark.

## Priorities, highest first
1. Measurement correctness
2. Benchmark fairness
3. Reproducibility
4. Evaluation coverage
5. Performance
6. Maintainability
7. Research innovation

Resolve conflicts in that order; easy but wrong numbers are not progress.

## Independence
- Never favour Fission or any other decompiler. FreshBench belongs to the fission-systems
  organisation; that is not evidence about any tool.
- Do not add decompiler-specific scoring, parsing, compilation or repair paths, or hardcode
  benchmark results.
- Public rankings contain only tools registered in `ci/tools.json` that are maintained at
  publication time. LLM-based decompilers are not ranked.
- No corpus binary is executed. The benchmark is static analysis only.

## Repository state
- v2 is the only line. The DecBench-based v1 release, its tag and its pipeline were removed from
  history; do not reintroduce them. v1 and v2 numbers are not comparable.
- `crates/freshbench/` is the v2 scorer and CLI. `crates/rust-joern/` is vendored dependency
  code, not FreshBench-owned code.
- Legacy v1 row readers in `crates/freshbench/src/rows.rs` and `dataset.rs` still exist. Do not
  extend them; remove them only in a dedicated change.
- Corpus files and results live on Hugging Face at pinned revisions. Only manifests, hashes and
  documentation are committed here.

## Hard invariants
### Evaluation population and failures
- The pinned corpus manifest and `truth/` data define the evaluation population. Decompiler
  output files never define the denominator.
- A missing result, crash, timeout, parse failure or unsupported target must stay distinguishable
  and must not silently disappear. It is not a perfect score.
- All tools are compared on the same rows. Use `report --corpus <corpus-root>` when a claim is
  about the whole corpus; a table built only from result files is not a complete evaluation.
- Partial evaluations must be reported as partial, never as complete.
### Ground truth
- Ground truth is generated independently of decompiler output and carries verifiable provenance.
  No decompiler is an unconditional oracle, including Ghidra, Fission, Kuna and angr.
- Never edit generated truth or corpus data by hand to improve a tool's score. Regenerate it from
  pinned provenance and document the new milestone.
- Missing source information must not be replaced with fabricated values. Ground-truth
  inconsistencies must be reported explicitly.
### GED
- Keep exact distances and approximate bounds distinguishable. For an exact score,
  `lower_bound == value`; otherwise `lower_bound < value`.
- Never let a lower bound exceed the reported upper bound. Search budgets count work, never
  wall-clock time.
- Isomorphism checks and graph hashes are optimisations unless equivalence is proven. Hash
  collisions must never produce a false perfect score.
- GED zero means structural equivalence only. It is not proof of correctness.
### Types
- Preserve architecture-specific data models (LP64 vs ILP32).
- Distinguish return types, ABI parameter types and local-variable coverage.
- Types are scored from the tool's evidence (`line_mappings`, `variables`, `return_type`) against
  DWARF, never from C text.
- Missing type evidence is not a perfect type score. Define applicability independently of a
  tool's output where possible; never reward missing evidence.
### Recompilation
- Compile output exactly as printed. No prelude, injected declarations, keyword rewriting,
  diagnostic-driven repair loop or tool-specific fixup.
- Compare every constant, displacement, register, operand and symbol/relocation target. Different
  external call targets are not equivalent merely because both addresses are relocated; an
  unsupported relocation must not produce an exact match.
- Compilation success is not semantic equivalence, and a byte-identical function is not proof
  that the program is correct.
### Determinism, cache and provenance
- Identical inputs produce byte-identical rows and reports on any machine; output must not depend
  on `--jobs` or thread scheduling.
- A score cache key includes the scorer identity, metrics revision, budget, function/source
  identity and output hash. Never reuse a score across scorers or data models.
- `load_table` must continue to refuse mixed scorers, conflicting duplicate rows and stale
  recompile side files.
- Every published number must carry the provenance of the run that produced it.
### Tests
- Every fixed scoring or evaluation defect gets a regression test. DecBench parity defects use
  the `decbench_NNN_*` naming convention.
- Never remove, skip or weaken a test merely to make CI pass.
- Never change expected values to accommodate a bug. Fix the implementation or document a
  deliberate new definition and update all affected snapshots.

## Mandatory workflow
1. **Inspect.** Check the branch and working tree. Read this file and the nearest nested
   `AGENTS.md`. Read the implementation and tests. Search for open issues, PRs and duplicate
   implementations. Do not overwrite unrelated user changes.
2. **Reproduce.** For a bug, identify the root cause, build a minimal reproducer, add a failing
   regression test when possible, and confirm it fails for the expected reason. Do not change
   production code for a merely plausible bug.
3. **Plan.** State the exact problem, affected modules, required invariants, expected behaviour,
   required tests and compatibility risks. Choose the smallest complete solution.
4. **Implement.** Follow existing conventions. Avoid unrelated refactoring and unjustified
   dependencies. Preserve public interfaces where possible. Do not silently weaken validation.
   Never introduce decompiler-specific exceptions.
5. **Validate.** Run the commands below and report the real result. A test that was not run is
   not a passing test.
6. **Review.** Check the final diff for correctness, regressions, benchmark bias, determinism,
   performance, cache invalidation, error handling, backward compatibility and unnecessary
   changes.
7. **Submit.** For future implementation tasks: use a dedicated branch, a descriptive commit and
   a focused PR explaining problem, implementation, evidence, limitations and validation. Do not
   merge, force-push or rewrite shared history without authorisation. A task is not complete
   merely because the code compiles.

## Validation
Run from the repository root. These are the repository-supported gates:

```bash
cargo fmt -p freshbench --check
cargo clippy --locked -p freshbench --all-targets -- -D warnings
cargo test --locked -p freshbench
```

CI (`.github/workflows/test.yml`) additionally runs:

```bash
cargo test --locked --release -p rust-joern
cargo test --locked -p freshbench elf_end_to_end -- --nocapture 2>&1 | tee e2e.log
grep -q "cc is gcc = true" e2e.log
```

The last two steps are Linux-only: on macOS the `elf_end_to_end` test is compiled out, so that CI
gate cannot be claimed locally. There is no Python test suite for the v2 harness. The vendored
crate's optional Python suite is described in `crates/rust-joern/AGENTS.md`.

When downloaded corpus data is available, validate it rather than trusting it:

```bash
target/release/freshbench manifest verify data/m1 corpus/m1/manifest.jsonl.gz
```

Report every check as **Passed**, **Failed**, **Skipped**, **Not executed** or **Blocked**, with
the reason. Do not claim a check passed unless it ran.

## Directory guides
- `crates/freshbench/AGENTS.md` — scorer, CLI, module boundaries, metric code
- `crates/rust-joern/AGENTS.md` — vendored parser and CFG crate
- `adapters/AGENTS.md` — decompiler adapter contract
- `corpus/AGENTS.md` — pinned corpus and manifests
- `selection/AGENTS.md` — candidate selection policy
- `ci/AGENTS.md` — pins, setup and qualification scripts
- `.github/AGENTS.md` — workflows and publication
- `docs/AGENTS.md` — compatibility audit documentation

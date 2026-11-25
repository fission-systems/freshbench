# AGENTS.md — `crates/freshbench/`

Root `AGENTS.md` applies. This crate is the v2 scorer and CLI; its output is the benchmark.

## Module boundaries

| Module | Responsibility |
|---|---|
| `csyntax.rs`, `cfg.rs` | Parse C and build the one CFG used for both source and output |
| `ged.rs` | Deterministic graph edit distance, exactness, lower bounds, isomorphism |
| `joern.rs` | Second CFG family from vendored `rust-joern`, scored by `ged.rs` |
| `evidence.rs`, `types.rs`, `dwarf.rs` | Line mappings/variables and DWARF type classes per data model |
| `recompile.rs` | Compile output as printed; relocation-aware instruction comparison |
| `score.rs`, `dataset.rs`, `rows.rs` | Row contract, score cache, scorer identity, table loading |
| `prepare.rs`, `corpus.rs`, `manifest.rs`, `provenance.rs` | Ground truth, corpus layout, hashes, run provenance |
| `adapters.rs`, `boundaries.rs`, `callsites.rs`, `speed.rs`, `sweep.rs` | Tool execution and secondary tracks |
| `report.rs`, `bands.rs`, `headroom.rs`, `taxonomy.rs`, `policy.rs` | Aggregation and publication policy |
| `main.rs` | CLI surface; keep commands, flags and help text consistent with behaviour |

## Local rules

- Do not change `cfg.rs` and `joern.rs` as if they were the same definition. They are two
  reported graph families; keep both and keep their scores separate.
- Types come only from `evidence.rs` data checked against DWARF. Never parse a type out of the
  output's C text. An output without valid line mappings is not scored.
- No fixups anywhere: no prelude, no injected declarations, no keyword rewriting, no repair loop,
  no per-tool special case in parsing, scoring or recompilation.
- `report --corpus <corpus-root>` is the complete-denominator path. Do not replace it with a
  denominator derived from result files. Do not add a way around `load_table`'s rejection of
  mixed scorers, conflicting rows or stale recompile side files.
- `build.rs` hashes the files in `SCORING` plus all of `crates/rust-joern/src` into
  `score::SCORER`. If a change affects CFG, GED, Joern, type or source-ground-truth scores,
  confirm the module is covered; add it if not. Recompile side files are protected by their
  recorded `code_sha256`, not by `SCORER`, so regenerate them after changing `recompile.rs`.
- Keep output deterministic: sorted keys, fixed float formatting, no timestamps. Behaviour must
  not depend on `--jobs` or Rayon scheduling.
- Keep v1 rows readable, but never mix v1 and v2 numbers in one table. Preserve the row JSON
  fields unless a migration is part of the task.
- Preserve public CLI compatibility where possible. A new required flag or renamed JSON field
  needs a reason and updated usage/docs.
- Return errors with context; do not add `unwrap`/`expect` to production paths. A scorer panic
  must cost one row, not the run.

## Telemetry (`sentry` feature)

- `src/telemetry.rs` is compiled into the binary only, never into the library. It is off unless
  the feature is on and `FRESHBENCH_SENTRY=1` and `SENTRY_DSN` are set.
- Its `scrub` function is the last line of defence before an event leaves the process. It
  rebuilds the event from an allowlist (exception type, `location` tag, fingerprint
  `[type, location]`, clean frames, release, environment); every other field is dropped.
  Message, log entry and exception value are templates, never the original text.
- Paths survive only as `crates/...`, `library/...` or `deps/<crate>-<ver>/...`
  (`clean_source_path`); frame function names only for the crates in `FRAME_CRATES`.
- Only panics are reported today, with the panic message never read. Expected validation
  failures must not be reported; infrastructure failure categories are a separate change.

## Tests

- Put unit tests in the module under `#[cfg(test)]`; integration regressions go in
  `tests/decbench_regressions.rs` or a focused file there. Fixtures belong in `tests/fixtures/`.
- Every scoring defect gets a test that fails before the fix. Use `decbench_NNN_*` for defects
  listed in `docs/decbench-compatibility-audit.md`.
- Prefer a small in-memory or temporary-file fixture over a downloaded corpus. Tests must not
  require network access.
- Run from the repository root:

```bash
cargo fmt -p freshbench --check
cargo clippy --locked -p freshbench --all-targets -- -D warnings
cargo test --locked -p freshbench
```

- CI also enforces the Linux-only ELF test:

```bash
cargo test --locked -p freshbench elf_end_to_end -- --nocapture 2>&1 | tee e2e.log
grep -q "cc is gcc = true" e2e.log
```

On macOS `elf_end_to_end` is compiled out; report that gate as not executed, never passed.
There is no Python test suite for this crate. The vendored crate has its own tests; see
`../rust-joern/AGENTS.md`.

## Known traps

- A Cargo warning says the non-root `rust-joern` profile is ignored. It is expected; do not
  "fix" profiles as unrelated cleanup.
- `SCORER` changes when a hashed scoring source changes, so old cached scores must not be reused.
  Rescore instead of forcing a cache hit.
- A missing line mapping, missing variable evidence or missing source definition must remain a
  distinct non-perfect outcome, not a zero-distance score.
- `report` and `gaps` only mean what they say when all tools share the same rows and scorer.

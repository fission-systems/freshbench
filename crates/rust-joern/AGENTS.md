# AGENTS.md — `crates/rust-joern/`

Root `AGENTS.md` applies. This directory is a vendored copy of
[Noelo-Lab/rust-joern](https://github.com/Noelo-Lab/rust-joern) (Apache-2.0), not FreshBench-owned
code.

## What this copy is

- `VENDORED.md` records the upstream base commit, the integrated PRs and the rule for replacing
  the copy.
- `INTEGRATION.md` records every merge, conflict resolution and follow-up commit.
- `docs/agents.md` is the upstream maintainer guide for the crate's internals, parser recovery
  rules, CFG/PyJoern compatibility and test commands. Read it before changing parser, builder,
  normalization or dataflow code.
- The upstream guide's note about top-level `AGENTS.md`/`CLAUDE.md` symlinks describes upstream,
  not this repository. This file is the active local guide.

## Change policy

- Do not patch this crate in place for a FreshBench-only need. Fix the behaviour upstream, or
  update the vendored snapshot as a whole, then update `VENDORED.md`, `INTEGRATION.md` and
  `NOTICE`.
- Do not silently diverge from the recorded upstream state. Any local change must be explicit,
  justified and recorded where the next re-vendor will find it.
- Keep Joern compatibility: this crate's purpose is Joern-compatible graphs, not a new graph
  definition. Intentional differences belong in `docs/divergences.md` with their rationale.
- A change to any file under `src/` changes FreshBench's `score::SCORER` because
  `crates/freshbench/build.rs` hashes all of it plus this crate's `Cargo.toml`. Old cached scores
  become invalid; rescore affected rows. Do not work around the scorer identity.
- FreshBench uses this crate for one CFG family only. Do not make FreshBench's `cfg.rs` definition
  and this Joern definition converge by weakening either one.

## Verification

From the repository root:

```bash
cargo test --locked --release -p rust-joern
cargo fmt -p rust-joern --check
cargo clippy --locked -p rust-joern --all-targets -- -D warnings
```

CI runs the release test but not the format/clippy commands for this crate; run them for any
change here and report what actually ran. The upstream guide lists optional Python tests; they
require a built shared library and are not part of FreshBench CI. Report environment skips
explicitly.

A Cargo warning that this crate's `[profile.release]` is ignored in the workspace is expected.
Do not change workspace or crate profiles as unrelated cleanup.

# AGENTS.md — `.github/`

Root `AGENTS.md` applies. Workflows build the corpus, run tools and publish pages; changes here
can invalidate published numbers.

## Required gate

`workflows/test.yml` is the test gate on pushes to `main` and pull requests. Keep all of these
steps:

- `cargo fmt -p freshbench --check`
- `cargo clippy --locked -p freshbench --all-targets -- -D warnings`
- `cargo test --locked -p freshbench`
- `cargo test --locked --release -p rust-joern`
- the Linux `elf_end_to_end` run followed by `grep -q "cc is gcc = true"`

Do not delete, skip, continue-on-error or weaken a step to get a PR green. If a step cannot run,
say so and fix the cause.

## Workflow roles

- `qualify-arch.yml`, `evaluate.yml`, `image-cross.yml` and `pages.yml` are the v2 path.
- `probe.yml` is a diagnostic. It is not a benchmark result.
- Benchmark build/evaluation workflows are manual `workflow_dispatch` because they are expensive
  and need pinned inputs. Do not make them run automatically on every push or PR.

## Publication

- Pages are built from `evaluate`/`boundaries` artifacts and each run's own `provenance.json`.
  Use the run's pins, not the current `ci/pins.env`, for page metadata.
- Keep the publication policy: registered, maintained, non-LLM tools only, with the deduplicated
  view and the boundary/speed data. Do not hand-write a number.
- Do not publish a table that mixes scorers, omits rows silently, or claims a complete evaluation
  from an incomplete artifact set. Preserve the full denominator and the failure categories.
- Keep `permissions` least-privilege and do not add write access where read access suffices.

## Review

No workflow linter is configured. Review YAML changes carefully and, when possible, run the
workflow on one small project/build before merging. Do not raise `max-parallel` or remove the
concurrency comments without checking the organisation's CI capacity.

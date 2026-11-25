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
- The ranking deploys automatically on a push to `main` that changes `ci/leaderboard_runs.json`.
  That file is the publication input: review its diff like a publication decision, and make sure
  each listed run is a completed `evaluate` run of the pinned M1 corpus.
- The `plan` job decides what runs. `build` and `deploy` run only when the gate has a run list
  and `deploy` is set. Do not add a path that deploys without the gate in `build`.
- Keep the publication policy: registered, maintained, non-LLM tools only, with the deduplicated
  view and the boundary/speed data. Do not hand-write a number.
- Do not publish a table that mixes scorers, omits rows silently, or claims a complete evaluation
  from an incomplete artifact set. Preserve the full denominator and the failure categories.
- Keep `permissions` least-privilege and do not add write access where read access suffices.

## Review

`workflows/workflow-lint.yml` runs actionlint and zizmor (high severity fails) on every PR. It is
advisory, not a required check. Run it locally before pushing a workflow change:

```bash
actionlint -shellcheck= -pyflakes=
pipx run zizmor==1.32.0 --offline --min-severity high --config .github/zizmor.yml .github/workflows
```

`.github/zizmor.yml` has no suppressions. Fix a finding in the workflow instead of ignoring it:
pin actions to a full commit SHA with a version comment, and pass `inputs.*`, `matrix.*` and
`github.*` values to `run:` through `env:`. A linter pass does not replace review: when possible, run the workflow on one small project/build before merging. Do not raise
`max-parallel` or remove the concurrency comments without checking the organisation's CI capacity.

## Labels

- `labels.json` is the managed label catalog. Labels are additive: never delete, rename or
  recolor an existing label from automation. `ci/sync_github_labels.py` (or the
  `sync-managed-labels` workflow, `apply=false` to preview) creates the missing ones only.
- `area:*` labels are added to PRs from changed paths (`labeler.yml`, `label-areas.yml`); add an
  area the paths miss by hand. Fork and Dependabot PRs are not auto-labeled.
  `python -m unittest ci.test_labeler` checks every glob matches a tracked file.
- `priority:P0`-`P3`, `status:*`, `type:*` and `risk:*` are manual. Use `risk:semantic` for any
  change that moves `score::SCORER`, a pin or a metric definition, and `risk:fairness` for anything
  that can favour one tool.

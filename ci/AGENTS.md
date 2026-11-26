# AGENTS.md — `ci/`

Root `AGENTS.md` applies. These scripts install pinned tools and build corpus inputs; a version
change is a benchmark change.

## Pins and registry

- `ci/pins.env` is the version contract for the corpus, compilers, build images, decompilers and
  DecBench/rust-joern. Do not bump a pin to make a build pass. Change it only with a stated reason,
  the affected milestone/run, and a plan to regenerate invalidated results.
- `ci/tools.json` is the publishable-tool registry. A new tool needs a maintained upstream
  repository, a working adapter, an evaluation path and a registration entry. LLM-based
  decompilers are not added to the public ranking.
- Each non-Fission tool's `version` in `ci/tools.json` names the version provenance records, with
  `{KEY}` for a pin of `ci/pins.env`. It lives in the registry, not in Rust, so registering a tool
  does not change the scorer identity. `provenance::tests` checks every rule against the pins.
- Never replace a pinned commit with a branch, tag or "latest".
- `ci/leaderboard_runs.json` lists the `evaluate` run IDs the public ranking is built from. A push
  to `main` that changes it publishes the ranking, so change it only with a reviewed PR.
- A full M1 sweep is `sweep.yml` (manual dispatch on `main`): it tags the commit `sweep/<id>`,
  dispatches every `evaluate` run from that tag as `ci/sweep_plan.py` splits them, and records the
  run IDs in the artifact `sweep-<id>`. Each run asks `sweep-collect.yml` to check the sweep when
  it ends (an hourly schedule does too); once all are done it runs the publication gate
  (`ci/collect_runs.sh`, shared with `pages.yml`), and publishes when it passes (dispatches
  `pages.yml` with `deploy=true`) and records the run list in an issue, otherwise it opens an issue
  with the refusal. A tool whose registry entry has `"decompile": false` (no evidence to score)
  runs in the boundary track only; one with `"sweep": false` is left out of sweeps. Manifold has
  it: rustc needs ~14 GB for its crate, and on a 16 GB runner the build is killed (with 16 GB of
  swap, the runner was lost after 50 minutes).
- **Concurrency**: one setting, sweep.yml's `total_parallel` (default `TOTAL_PARALLEL` = 40 in
  `ci/sweep_plan.py`), caps the evaluate jobs of a whole sweep, so other repositories of the
  organization keep ~20 of its 60 job slots.
- **Output reuse** (`crates/freshbench/src/reuse.rs`): a non-Fission job takes a binary's output
  from an earlier run when the key matches exactly (tool, pinned version, adapter files, the
  environment `ci/setup_tool.sh` records, harness sources, binary, requests, limits), instead of
  running the tool; every score is recomputed. Each job's `provenance.json` lists every binary's
  key and the run an output came from, and the publication gate refuses a reused output whose key
  differs, a reused Fission output, and a table mixing a tool's versions, adapters or harnesses.
  Outputs live in the runs' artifacts (90 days); a reused output is uploaded again, so a sweep
  within 90 days of the last keeps them. A new adapter, pin, harness or environment change is a miss.

## Current v2 path

- `ci/qualify_arch.sh` requires `owner/repo@full-sha`; it builds in the pinned cross image with
  no network, applies both O0 gates (at least 300 eligible functions each), then runs
  `freshbench prepare`. `FRESHBENCH_BIN` must be a Linux build used inside the image; do not
  substitute a host binary.
- `ci/setup_tool.sh` installs one decompiler from the pins and exports `FB_ADAPTER`.
  Adapters must not download or update a tool while a benchmark run is in progress.
- The `qualify-arch` and `evaluate` workflows are the v2 entry points. Their artifacts are inputs
  to results, so preserve the run's `provenance.json` and do not edit artifacts by hand.

`ci/setup_systemd_user.sh` gives `evaluate.yml` the user systemd that `ci/capped.sh` needs: every
tool runs under a 14 GB memory cap without swap, so running out of memory costs the killed call,
not the runner and with it every row of the job. A job with such a kill uploads
`oom-kills.txt` and offers nothing for reuse. Review the script before adding another caller.

## Rules

- Never print, commit or pass secrets through script arguments. CI uses the workflow token only.
- Do not weaken a gate (minimum function count, offline build, SHA check, stripped-input check) to
  make a candidate qualify. Report the failure instead.
- A qualification run that fails is evidence. Preserve its `status.json` and logs; do not rewrite
  the status to a success.
- Changing a script must keep the workflow inputs and exported environment variables consistent
  with the calling workflow.

## Verification

There is no shell unit-test suite. For a script change, run `bash -n` on every changed shell
file, then exercise the relevant workflow on one project with the pinned image. If that cannot be
done, report the check as **Blocked**. Do not claim a qualification or evaluation passed from a
local syntax check alone.

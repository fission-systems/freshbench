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
- Never replace a pinned commit with a branch, tag or "latest".

## Current v2 path

- `ci/qualify_arch.sh` requires `owner/repo@full-sha`; it builds in the pinned cross image with
  no network, applies both O0 gates (at least 300 eligible functions each), then runs
  `freshbench prepare`. `FRESHBENCH_BIN` must be a Linux build used inside the image; do not
  substitute a host binary.
- `ci/setup_tool.sh` installs one decompiler from the pins and exports `FB_ADAPTER`.
  Adapters must not download or update a tool while a benchmark run is in progress.
- The `qualify-arch` and `evaluate` workflows are the v2 entry points. Their artifacts are inputs
  to results, so preserve the run's `provenance.json` and do not edit artifacts by hand.

`ci/setup_systemd_user.sh` is not used by any workflow. Do not add a caller without reviewing it.

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

# AGENTS.md — `selection/`

Root `AGENTS.md` applies. Selection is fixed before tools run; changing it after seeing results
invalidates the benchmark.

## Policy

- `enumerate.py` defines the candidate rule: GitHub primary language C, 50–3000 stars, pushed
  after the snapshot cutoff, not a fork/archive/template, OSI license, at most 200 MB. Results are
  ordered by `sha256("freshbench-v1/" + full_name.lower())`, not by preference.
- `exclude.py` applies the fixed DecBench and well-known-project name exclusions, including the
  documented substrings. Do not remove an exclusion because a project looks useful or because a
  tool scores well on it.
- `candidates.json` is generated output. Do not hand-edit it; `candidates_raw.json` is ignored by
  Git.
- `m1/m1.json` is the published 20-project list for M1. `m1/qualification.json` records the
  qualification evidence. `corpus100.json` is the candidate order M1 was drawn from; keep it.
- A project enters a corpus only after qualification: both O0 builds have at least 300 eligible
  functions, all builds succeed, and preparation selects the same sha256-ordered functions on
  every architecture. Never hand-pick functions or projects.
- Changing the snapshot, rule, exclusions or project list creates a new milestone. It does not
  silently revise M1.

## Commands

Regenerating candidates requires the GitHub API (`gh`) and network access; do it only when
intentionally refreshing the snapshot:

```bash
python3 selection/enumerate.py selection/candidates_raw.json
python3 selection/exclude.py selection/candidates_raw.json selection/candidates.json selection/excluded.json
```

Qualification runs through the `qualify-arch` workflow with the pinned cross image, not on a
developer machine. Do not replace the pinned commit with a branch or the latest revision.

## Review

When reviewing a selection change, verify that the order is hash-derived, exclusions are applied
before builds, every included project is pinned to a full commit, and any count in
`corpus/m1/README.md` was regenerated rather than adjusted by hand.

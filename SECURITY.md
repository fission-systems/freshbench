# Security policy

## Reporting a vulnerability

Report it privately via GitHub's
[private vulnerability reporting](https://github.com/fission-systems/freshbench/security/advisories/new)
(the repository's **Security** tab, then **Report a vulnerability**). Do not open a public issue,
pull request or discussion for a vulnerability.

Include:
- what is affected (file, workflow, command or published page) and the commit or run it was seen in;
- steps to reproduce, or a proof of concept;
- the impact as you understand it.

There is no fixed response schedule. Reports are acknowledged and handled on a best-effort basis;
please allow time for a fix before disclosing.

## Scope

In scope: code and configuration in this repository.

- **The benchmark harness:** the `freshbench` scorer and CLI (`crates/freshbench/`), the vendored
  `crates/rust-joern/` as FreshBench builds and uses it, and the scripts in `ci/` and `build/`.
- **Adapters** (`adapters/`) that run each decompiler, and how they install, invoke and read it.
- **CI workflows** (`.github/workflows/`), for example injection, token or permission problems,
  or a way to publish without the publication gate.
- **The published pages** (GitHub Pages for this repository) and the inputs they are built from.
- **Integrity of published results:** a way to change a score, ranking, corpus manifest or
  provenance record without that change being visible.

Out of scope:

- **Upstream decompilers** (Ghidra, angr, Fission, Kuna, Borer, Unlace and any other tool in
  `ci/tools.json`). Report those to their maintainers. A bug in how FreshBench installs or runs one
  of them is in scope.
- **Corpus projects:** vulnerabilities in the third-party source code compiled into the corpus.
  FreshBench never executes corpus binaries.
- Third-party services the project uses (GitHub, Hugging Face, Sentry) themselves.
- A disagreement with a benchmark score or method that is not a security issue. Open a normal
  issue for that.

## Supported versions

Only the current `main` branch is supported. There are no maintained release branches.

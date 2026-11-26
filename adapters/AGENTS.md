# AGENTS.md — `adapters/`

Root `AGENTS.md` applies. Adapters are part of the measurement boundary: a biased adapter makes
every downstream number biased.

## Contract

The canonical description is `adapters/README.md`. Preserve it:

- Decompile mode receives `--binary`, `--addresses`, `--elf-base`, `--out` and writes one JSON
  line per requested address, with the address exactly as requested.
- Every output must carry the tool's own `line_mappings`; report `variables` and `return_type` from
  the tool's evidence when the tool provides them. Empty evidence is a non-perfect type result,
  not a reason to invent types.
- Line mappings come from the tool while it prints; never derive them from C text, source positions
  or address ranges. An output with no line mapping to a valid instruction is not scored.
- Variables use the ABI argument position (`arg_index`) and the tool's stack convention
  (`stack_offset`). Do not invent types or reorder arguments to fit DWARF.
- List mode (`--list-functions`) reports only functions the tool's own analysis found. Do not use
  symbol files, FreshBench hints, `--define-function` or another tool's results.
- Every tool receives the same function starts. Do not add or remove addresses for one tool.
  On ARM, a requested Thumb function is `address | 1`.

## Rules

- An adapter is a translation layer, not an analysis layer. Do not repair, normalize, rewrite or
  complete decompiler output.
- Do not add tool-specific scoring exceptions, filtering that hides a tool's failures, or
  post-processing that makes one dialect compile.
- Keep shared code in `common/` tool-neutral. A change for Kuna must not change Unlace behaviour
  unless both are tested.
- Report a per-function failure in `error`; exit non-zero only when the tool could not analyse the
  binary at all. Keep diagnostics on stderr. One exception, documented in `revng/run`: on 32-bit
  ARM a failed `revng project init` is reported per function, since bisecting only repeats it.
- Respect the pinned versions in `ci/pins.env`. Do not download or update a tool at runtime.
- A new decompiler also needs a `ci/tools.json` entry, a maintained upstream repository and an
  evaluation path. LLM-based decompilers are not added to the public ranking.

## Verification

There is no adapter unit-test suite that runs real decompilers in CI. For an adapter change:

1. Run it on a small prepared tree with the pinned tool and inspect the JSON contract.
2. Confirm one line per requested address and that returned addresses match the request.
3. Confirm missing line mappings produce a non-scored result, not a fabricated mapping.
4. Run `cargo test --locked -p freshbench` for the Rust-side runner and retry/timeout behaviour.

If the real tool is unavailable, report the adapter check as **Blocked**; do not claim it passed.

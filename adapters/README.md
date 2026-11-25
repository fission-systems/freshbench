# Tool adapters

`freshbench decompile --tool <name> --adapter adapters/<name>/run` calls one of
these once per binary (and once per function after a failed call):

    run --binary <stripped ELF> --addresses <file> --elf-base 0x... --out <jsonl>

* `--addresses`: one `0x...` per line, ELF virtual addresses. On ARM, a Thumb
  function is given as `address | 1` (the ELF-for-ARM convention).
* `--elf-base`: the lowest PT_LOAD address, page-aligned, so an adapter whose
  tool rebases the image can translate addresses.
* `--out`: one JSON line per requested address, with the address written
  exactly as requested: `{"address": "0x...", "code": "<C text>" | null, "error": null | "..."}`.

An adapter only translates between FreshBench and the tool. It adds no analysis
of its own, except that it creates a function at a requested address when the
tool's own analysis did not (Ghidra, angr). Every decompiler gets the same list
of function starts, as any user who knows a function's address would give it.

| tool | adapter | needs |
|---|---|---|
| ghidra | `ghidra/run` + `FreshBenchDecompile.java` (list mode: `FreshBenchFunctions.java`) | `GHIDRA_INSTALL_DIR`, JDK 21 |
| kuna | `kuna/run` (Kuna's own CLI: `decompile-project --addr ...`, per-function `decompile --addr --json` for the rest) | `KUNA_BIN` |
| angr | `angr/run` | Python with `angr` |
| unlace | `unlace/run` (`decompile-all --json` with `--addr` per function; list mode: `unlace functions --json`) | `UNLACE_BIN`, `UNLACE_SPECS`; built from the pinned commit, no binary releases |
| borer | `borer/run` (list mode: `borer functions` pages, then `borer function` per entry for its range; `BORER_LIST_JOBS` parallel calls, default CPU count) | `BORER_BIN` |
| fission | built in (`--fission-cli`) | `fission_cli` |

## Function boundaries (`freshbench boundaries`)

The boundary track gives a tool **no addresses**: it must find the functions
of a stripped binary itself. The same adapter is called in list mode:

    run --list-functions --binary <stripped ELF> --elf-base 0x... --out <jsonl>

* `--out`: one JSON line per function the tool found, ELF virtual addresses:
  `{"start": "0x...", "end": "0x..." | null}`. `end` is one past the last byte
  of the function's body (the highest address of any of its blocks), or null
  when the tool does not know it. A Thumb start may carry bit 0; it is
  cleared before scoring.
* Report what the tool's own analysis found, as a user would see it in its
  function list: no FreshBench hints, no symbol files, no `--define-function`.
  Leave out what the tool itself marks as not a function of the binary
  (PLT stubs, imported/external symbols, SimProcedures, alignment padding).
* Exit non-zero only when the tool could not analyze the binary at all.

Found starts inside PLT sections are not scored for any tool (some tools
list PLT stubs, others filter them). Kuna's `size` reaches to the next entry,
padding included, so its ends count as padded, rarely exact.

Ground truth comes from the unstripped binary: every `STT_FUNC` symbol with a
size in an executable section, and every DWARF subprogram with a range
(`boundaries.rs`).

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
| ghidra | `ghidra/run` + `FreshBenchDecompile.java` | `GHIDRA_INSTALL_DIR`, JDK 21 |
| kuna | `kuna/run` (Kuna's own CLI: `decompile-project --addr ...`, per-function `decompile --addr --json` for the rest) | `KUNA_BIN` |
| angr | `angr/run` | Python with `angr` |
| borer | `borer/run` | `BORER_BIN` |
| fission | built in (`--fission-cli`) | `fission_cli` |

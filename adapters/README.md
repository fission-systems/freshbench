# Tool adapters

`freshbench decompile --tool <name> --adapter adapters/<name>/run` calls one of
these once per binary (and once per function after a failed call):

    run --binary <stripped ELF> --addresses <file> --elf-base 0x... --out <jsonl>

* `--addresses`: one `0x...` per line, ELF virtual addresses. On ARM, a Thumb
  function is given as `address | 1` (the ELF-for-ARM convention).
* `--elf-base`: the lowest PT_LOAD address, page-aligned, so an adapter whose
  tool rebases the image can translate addresses.
* `--out`: one JSON line per requested address, with the address written
  exactly as requested:

  ```json
  {"address": "0x...", "code": "<C text>" | null, "error": null | "...",
   "line_mappings": [{"line_number": 3, "addresses": [4198400, "0x401004"]}],
   "variables": [{"name": "p", "type": "char *", "kind": "arg|stack|register",
                  "arg_index": 0 | null, "stack_offset": -16 | null, "size": 8,
                  "addresses": [4198400]}],
   "return_type": "int" | null}
  ```

**Line mappings are required** (DecBench's native line-mapping contract,
Noelo-Lab/decbench#129). `line_mappings` say which instructions (ELF virtual
addresses) each 1-based line of `code` came from, as the tool itself recorded
them while printing; never derive them from the text, from source positions or
from address ranges. FreshBench keeps only addresses that are instruction
starts inside the function (x86 decoded, ARM aligned), and **an output with no
line that maps to such an instruction is not scored at all** (`no_line_mappings`).

Types are scored only from `variables` and `return_type`, never from the C
text: parameters by `arg_index` (the ABI position, not the order the tool lists
them in), locals by `stack_offset` (any constant frame convention; one shift is
calibrated per function). A tool that reports no variables has no type score.

An adapter only translates between FreshBench and the tool. It adds no analysis
of its own, except that it creates a function at a requested address when the
tool's own analysis did not (Ghidra, angr). Every decompiler gets the same list
of function starts, as any user who knows a function's address would give it.

| tool | adapter | needs |
|---|---|---|
| ghidra | `ghidra/run` + `FreshBenchDecompile.java` (the C is rendered from Ghidra's token lines, so line N carries its tokens' addresses; variables from the HighFunction's symbols; list mode: `FreshBenchFunctions.java`) | `GHIDRA_INSTALL_DIR`, JDK 21 |
| kuna | `kuna/run` (`common/kuna_family.py`: `decompile-all --json` with the given starts, which carries Kuna's own line mappings and variables; `decompile --addr --json` for the rest) | `KUNA_BIN` |
| angr | `angr/run` (line mappings from the codegen position map, arguments from `cfunc.arg_list`, locals with stack offsets) | Python with `angr` |
| unlace | `unlace/run` (the same `common/kuna_family.py`; list mode: `unlace functions --json`) | `UNLACE_BIN`, `UNLACE_SPECS`; built from the pinned commit, no binary releases |
| borer | `borer/run` (borer's CLI prints no line-to-address mapping and no variables, so its decompiled output is not scored; it takes part in the boundary track; list mode: `borer functions` pages, then `borer function` per entry for its range; `BORER_LIST_JOBS` parallel calls, default CPU count) | `BORER_BIN` |
| fission | built in (`--fission-cli`) | `fission_cli` |
| ghidra-rs | `ghidra-rs/run` + `ghidra-rs/src/main.rs` (koute/ghidra-decompiler, a pure-Rust port of Ghidra 12.2's C++ decompiler, used as a library; the printer's markup calls go to a token sink, so `code` and line N's addresses come from the same token lines as the ghidra adapter's, and a function whose token rendering differs from the library's plain text gets an `error`; variables from the function's local scope, types in the C++ decompiler's own names such as `int4`; ARM: TMode set at each requested start; list mode: `discover_functions()`, `end` from the library's own flow) | `GHIDRA_RS_BIN`; its own Cargo project (not in the workspace, so the scorer identity does not depend on it), crate pinned in its `Cargo.lock` |
| revng | `revng/run` (`revng project init`, then `artifact emit-c`: every C fragment rev.ng lifts from machine code carries `/instruction/<fn>/<block>/<insn>` in its PTML `data-action-context-location`, so line N carries the instructions of the fragments printed on it; parameters from the prototype's `/cabi-argument/<proto>/<index>` spans (raw register prototypes have no ABI position and are left out), return type from `/return-value/`, stack locals from the stack-frame struct's `/struct-field/<type>/<offset>` spans; a requested start rev.ng did not find is added to the model and its analysis rerun; a dynamic object is loaded at rev.ng's base and translated back; list mode: the model's `Functions`, ends from `emit-cfg`) | `REVNG_BIN`, PyYAML; the pinned binary distribution (no releases) |
| r2sleigh | `r2sleigh/run` (`r2s` once per binary, commands on stdin: `pddj` for r2sleigh's own line mappings and variables, `pdd` for the definition shown to a user, with `pddj`'s line numbers moved up by its translation-unit prelude as r2s does; r2sleigh renders C for x86-64 and AArch64 only and refuses x86 32-bit and ARM 32-bit, recorded as the error; no return type outside the signature text; list mode: `aaa; aflj`, `sym.imp.*` left out, ends from `afb`) | `R2S_BIN`; built from the pinned commit, no binary releases |
| reko | `reko/run` (Reko prints no instruction address on any statement of its C output and no machine-readable signatures, so its decompiled output is not scored; it takes part in the boundary track. Decompile: a `.dcproject` declares the given starts as user procedures; on 32-bit ARM Thumb starts cannot be declared (Reko decodes user procedures as A32), so only Thumb procedures Reko found itself are answered. List mode: `disassemble`, Reko's default scan, the `proc` headers of its `.asm`; ends from the `.dis` blocks) | `REKO_DIR` (pinned CmdLine zip), `REKO_DOTNET` (pinned .NET 8 runtime) |

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

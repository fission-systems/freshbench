"""Call-site agreement: calls in the machine code vs calls in the decompiled text.

The ground truth is the unstripped binary, not the source: inlining removes
source calls and the compiler adds none that the source lacks except runtime
helpers (excluded below), so the direct call instructions of a function are
what a faithful decompilation prints, once each.

Binary side: `llvm-objdump -d --no-show-raw-insn` of the unstripped binary.
Every direct call (`call`, `bl`, `blx`) whose target is a symbol start is a
call site; a direct jump (`jmp`, `b`, `b.w`) to the start of *another*
function is a tail call and counts too. Indirect calls are not counted on
either side.

Output side: callee identifiers in the decompiled text (ctext.features).
Address-style names (FUN_00401136, sub_401136, func_0x401136, fcn.00401136)
are mapped to the unstripped binary's symbol at that address, so every tool's
naming convention compares against the same names.

Scores per row: binary call count, matched (multiset intersection),
missing = binary - matched (code the output does not show), excess =
output - matched (duplicated or invented calls). A static agreement signal,
not a correctness proof: it cannot see wrong arguments.
"""
from __future__ import annotations

import re
from collections import Counter
from dataclasses import dataclass

CALL_MNEMONICS = frozenset({"call", "callq", "calll", "callw", "bl", "blx"})
TAIL_MNEMONICS = frozenset({"jmp", "jmpq", "jmpl", "b", "b.w"})
RUNTIME_PREFIXES = (
    "__x86.get_pc_thunk", "__stack_chk_fail", "__aeabi_", "__udivdi3", "__divdi3", "__umoddi3",
    "__moddi3", "__udivmoddi4", "__divmoddi4", "__fentry__", "mcount", "__gnu_mcount_nc",
    "__cyg_profile_", "__tls_get_addr", "___tls_get_addr", "__chkstk", "__morestack",
    "__clear_cache", "__sanitizer_", "__asan_", "__ubsan_",
)

_HEADER = re.compile(r"^([0-9a-fA-F]+) <(.+)>:\s*$")
# requires --no-show-raw-insn: raw bytes such as "add" or "bl" are valid hex
_INSN = re.compile(r"^\s*([0-9a-fA-F]+):\s+([A-Za-z][\w.]*)\s*(.*)$")
_TARGET = re.compile(r"(?:0x)?([0-9a-fA-F]+) <([^>]+?)(\+0x[0-9a-fA-F]+)?>")
_ADDR_NAME = re.compile(
    r"^(?:FUN_|sub_|func_|fcn_|fcn\.|SUB_|LAB_|thunk_FUN_)(?:0x)?([0-9a-fA-F]{3,16})$")


def is_runtime(name: str) -> bool:
    return name.startswith(RUNTIME_PREFIXES)


def clean_symbol(sym: str) -> str:
    for suffix in ("@plt", "@PLT"):
        if sym.endswith(suffix):
            sym = sym[: -len(suffix)]
    return sym.split("@", 1)[0]  # versioned names: memcpy@GLIBC_2.14


@dataclass
class FunctionCalls:
    name: str
    start: int
    calls: Counter  # callee name -> count


def parse_objdump(text: str) -> dict[int, FunctionCalls]:
    """Direct calls of every function in `llvm-objdump -d` output, by start address."""
    funcs: dict[int, FunctionCalls] = {}
    cur: FunctionCalls | None = None
    for line in text.splitlines():
        h = _HEADER.match(line)
        if h:
            addr = int(h.group(1), 16)
            cur = funcs.get(addr)
            if cur is None:
                cur = funcs[addr] = FunctionCalls(h.group(2), addr, Counter())
            continue
        if cur is None:
            continue
        m = _INSN.match(line)
        if not m:
            continue
        mnem = m.group(2).lower()
        is_call = mnem in CALL_MNEMONICS
        is_tail = mnem in TAIL_MNEMONICS
        if not (is_call or is_tail):
            continue
        t = _TARGET.search(m.group(3))
        if not t or t.group(3):  # no symbol, or symbol+offset: not a function start
            continue
        callee = clean_symbol(t.group(2))
        if is_tail and callee == clean_symbol(cur.name):
            continue  # a jump to its own start is a loop, not a call
        if is_runtime(callee):
            continue
        cur.calls[callee] += 1
    return funcs


def symbol_table(funcs: dict[int, FunctionCalls]) -> dict[int, str]:
    return {a: clean_symbol(f.name) for a, f in funcs.items()}


def normalize_callee(name: str, addr2name: dict[int, str], delta: int = 0) -> str:
    """A decompiler's callee identifier as the unstripped binary's symbol name.

    `delta` is the tool's address-space offset (a tool that rebases PIE images
    prints addresses shifted by it); address-style names off any symbol stay
    as they are.
    """
    name = clean_symbol(name)
    m = _ADDR_NAME.match(name)
    if m:
        addr = int(m.group(1), 16) - delta
        return addr2name.get(addr, name)
    return name


@dataclass(frozen=True)
class Agreement:
    binary: int
    output: int
    matched: int

    @property
    def missing(self) -> int:
        return self.binary - self.matched

    @property
    def excess(self) -> int:
        return self.output - self.matched

    @property
    def exact(self) -> bool:
        return self.missing == 0 and self.excess == 0

    def as_dict(self) -> dict:
        return {"binary_calls": self.binary, "output_calls": self.output, "matched": self.matched,
                "missing": self.missing, "excess": self.excess, "exact": self.exact}


def compare(binary_calls: Counter, output_calls: Counter, addr2name: dict[int, str],
            delta: int = 0) -> Agreement:
    out = Counter()
    for name, n in output_calls.items():
        norm = normalize_callee(name, addr2name, delta)
        if not is_runtime(norm):
            out[norm] += n
    matched = sum((binary_calls & out).values())
    return Agreement(sum(binary_calls.values()), sum(out.values()), matched)

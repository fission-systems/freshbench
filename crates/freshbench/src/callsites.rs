//! Call-site agreement: calls in the machine code vs calls in the decompiled text.
//!
//! Ground truth is the unstripped binary, not the source: inlining removes
//! source calls, so the direct call instructions of a function are what a
//! faithful decompilation prints, once each. Compiler runtime helpers (PIC
//! thunks, stack-protector failure, ARM EABI division, ...) are excluded on
//! both sides.
//!
//! Binary side: `llvm-objdump -d --no-show-raw-insn` of the unstripped binary.
//! Every direct call (`call`, `bl`, `blx`) to a symbol start is a call site;
//! a direct jump (`jmp`, `b`, `b.w`) to the start of another function is a
//! tail call and counts too. Indirect calls are counted on neither side.
//!
//! Output side: callee names in the decompiled text. Address-style names
//! (`FUN_00401136`, `sub_401136`, `func_0x401136`, `fcn.00401136`) map to the
//! unstripped symbol at that address. A static agreement signal: it cannot see
//! wrong arguments.

use std::collections::BTreeMap;

use serde::Serialize;

pub const CALL_MNEMONICS: [&str; 6] = ["call", "callq", "calll", "callw", "bl", "blx"];
pub const TAIL_MNEMONICS: [&str; 5] = ["jmp", "jmpq", "jmpl", "b", "b.w"];
pub const RUNTIME_PREFIXES: [&str; 22] = [
    "__x86.get_pc_thunk",
    "__stack_chk_fail",
    "__aeabi_",
    "__udivdi3",
    "__divdi3",
    "__umoddi3",
    "__moddi3",
    "__udivmoddi4",
    "__divmoddi4",
    "__fentry__",
    "mcount",
    "__gnu_mcount_nc",
    "__cyg_profile_",
    "__tls_get_addr",
    "___tls_get_addr",
    "__chkstk",
    "__morestack",
    "__clear_cache",
    "__sanitizer_",
    "__asan_",
    "__ubsan_",
    "__stack_chk_guard",
];

pub type Calls = BTreeMap<String, u32>;

pub fn is_runtime(name: &str) -> bool {
    RUNTIME_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// `puts@plt` -> `puts`, `memcpy@GLIBC_2.14` -> `memcpy`.
pub fn clean_symbol(sym: &str) -> &str {
    sym.split('@').next().unwrap_or(sym)
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionCalls {
    pub name: String,
    pub start: u64,
    pub calls: Calls,
}

fn parse_hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// `<sym>` or `<sym+0x10>` after an address: (symbol, has offset).
fn target(operands: &str) -> Option<(&str, bool)> {
    let open = operands.find('<')?;
    let close = operands[open..].find('>')? + open;
    let inner = &operands[open + 1..close];
    match inner.rfind("+0x") {
        Some(i) => Some((&inner[..i], true)),
        None => Some((inner, false)),
    }
}

/// Direct calls of every function in `llvm-objdump -d --no-show-raw-insn` output, by start address.
pub fn parse_objdump(text: &str) -> BTreeMap<u64, FunctionCalls> {
    let mut funcs: BTreeMap<u64, FunctionCalls> = BTreeMap::new();
    let mut cur: Option<u64> = None;
    for line in text.lines() {
        // header: "0000000000401136 <main>:"
        if let Some(rest) = line.strip_suffix(">:") {
            if let Some((addr, name)) = rest.split_once(" <") {
                if let Some(a) = parse_hex(addr.trim()) {
                    funcs.entry(a).or_insert_with(|| FunctionCalls {
                        name: name.to_string(),
                        start: a,
                        calls: Calls::new(),
                    });
                    cur = Some(a);
                    continue;
                }
            }
        }
        let Some(start) = cur else { continue };
        // instruction: "  401137:      \tcallq\t0x401020 <puts@plt>"
        let Some((addr, rest)) = line.split_once(':') else {
            continue;
        };
        if parse_hex(addr.trim()).is_none() || addr.trim().is_empty() {
            continue;
        }
        let mut it = rest.split_whitespace();
        let Some(mnem) = it.next() else { continue };
        let mnem = mnem.to_ascii_lowercase();
        let is_call = CALL_MNEMONICS.contains(&mnem.as_str());
        let is_tail = TAIL_MNEMONICS.contains(&mnem.as_str());
        if !is_call && !is_tail {
            continue;
        }
        let operands: String = it.collect::<Vec<_>>().join(" ");
        if operands.starts_with('*') {
            continue; // x86 indirect: jmpq *0x2fe2(%rip) # <puts@GLIBC>
        }
        let Some((sym, has_off)) = target(&operands) else {
            continue;
        };
        if has_off {
            continue; // inside a function: not a call target
        }
        let callee = clean_symbol(sym);
        let f = funcs.get_mut(&start).unwrap();
        if is_tail && callee == clean_symbol(&f.name) {
            continue; // jump to its own start: a loop
        }
        if is_runtime(callee) {
            continue;
        }
        *f.calls.entry(callee.to_string()).or_default() += 1;
    }
    funcs
}

pub fn symbol_table(funcs: &BTreeMap<u64, FunctionCalls>) -> BTreeMap<u64, String> {
    funcs
        .iter()
        .map(|(a, f)| (*a, clean_symbol(&f.name).to_string()))
        .collect()
}

const ADDR_PREFIXES: [&str; 8] = [
    "thunk_FUN_",
    "FUN_",
    "sub_",
    "func_",
    "fcn_",
    "fcn.",
    "SUB_",
    "LAB_",
];

/// A decompiler's callee name as the unstripped binary's symbol name.
/// `delta` is the tool's rebase offset (printed address = real + delta).
pub fn normalize_callee(name: &str, addr2name: &BTreeMap<u64, String>, delta: i64) -> String {
    let name = clean_symbol(name);
    for p in ADDR_PREFIXES {
        if let Some(rest) = name.strip_prefix(p) {
            let hexpart = rest.strip_prefix("0x").unwrap_or(rest);
            if (3..=16).contains(&hexpart.len()) && hexpart.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Some(a) = parse_hex(hexpart) {
                    let real = (a as i64).wrapping_sub(delta) as u64;
                    if let Some(s) = addr2name.get(&real) {
                        return s.clone();
                    }
                }
            }
            break;
        }
    }
    name.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Agreement {
    pub binary_calls: u32,
    pub output_calls: u32,
    pub matched: u32,
    pub missing: u32,
    pub excess: u32,
    pub exact: bool,
}

pub fn compare(
    binary: &Calls,
    output: &Calls,
    addr2name: &BTreeMap<u64, String>,
    delta: i64,
) -> Agreement {
    let mut out = Calls::new();
    for (name, n) in output {
        let norm = normalize_callee(name, addr2name, delta);
        if !is_runtime(&norm) {
            *out.entry(norm).or_default() += n;
        }
    }
    let matched: u32 = binary
        .iter()
        .map(|(k, v)| (*v).min(out.get(k).copied().unwrap_or(0)))
        .sum();
    let b: u32 = binary.values().sum();
    let o: u32 = out.values().sum();
    Agreement {
        binary_calls: b,
        output_calls: o,
        matched,
        missing: b - matched,
        excess: o - matched,
        exact: b == matched && o == matched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const X86: &str = "
a.out:\tfile format elf64-x86-64

Disassembly of section .plt:

0000000000401020 <puts@plt>:
  401020:      \tjmpq\t*0x2fe2(%rip)           # 0x404008 <puts@GLIBC_2.2.5>

Disassembly of section .text:

0000000000401136 <helper>:
  401136:      \tpushq\t%rbp
  401137:      \tcallq\t0x401020 <puts@plt>
  40113c:      \tjne\t0x401150 <helper+0x1a>
  40113e:      \tcallq\t*%rax
  401140:      \tjmp\t0x401136 <helper>
  401143:      \tretq

0000000000401150 <main>:
  401150:      \tcallq\t0x401136 <helper>
  401155:      \tcallq\t0x401136 <helper>
  40115a:      \tcalll\t0x401200 <__x86.get_pc_thunk.bx>
  40115f:      \tcallq\t0x401020 <puts@plt>
  401164:      \tjmp\t0x401136 <helper>
";

    const A64: &str = "
0000000000000750 <compute>:
     750:      \tstp\tx29, x30, [sp, #-0x10]!
     754:      \tbl\t0x600 <memcpy@plt>
     758:      \tb.ne\t0x770 <compute+0x20>
     75c:      \tblr\tx8
     760:      \tb\t0x800 <tailed>
";

    const ARM: &str = "
00010400 <thumbfn>:
   10400:      \tpush\t{r7, lr}
   10402:      \tbl\t0x10300 <printf@plt>
   10406:      \tblx\tr3
   10408:      \tbl\t0x10500 <__aeabi_idiv>
   1040c:      \tb.w\t0x10600 <other>
";

    fn calls(pairs: &[(&str, u32)]) -> Calls {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn x86_64() {
        let f = parse_objdump(X86);
        assert_eq!(f[&0x401136].calls, calls(&[("puts", 1)]));
        assert_eq!(f[&0x401150].calls, calls(&[("helper", 3), ("puts", 1)]));
        assert!(f[&0x401020].calls.is_empty());
    }

    #[test]
    fn aarch64_and_arm() {
        assert_eq!(
            parse_objdump(A64)[&0x750].calls,
            calls(&[("memcpy", 1), ("tailed", 1)])
        );
        assert_eq!(
            parse_objdump(ARM)[&0x10400].calls,
            calls(&[("other", 1), ("printf", 1)])
        );
    }

    #[test]
    fn callee_styles() {
        let a2n = symbol_table(&parse_objdump(X86));
        for n in [
            "FUN_00401136",
            "sub_401136",
            "func_0x401136",
            "fcn.00401136",
            "helper",
        ] {
            assert_eq!(normalize_callee(n, &a2n, 0), "helper", "{n}");
        }
        assert_eq!(normalize_callee("sub_999999", &a2n, 0), "sub_999999");
        assert_eq!(normalize_callee("sub_501136", &a2n, 0x100000), "helper");
    }

    #[test]
    fn agreement() {
        let f = parse_objdump(X86);
        let a2n = symbol_table(&f);
        let out = calls(&[("sub_401136", 2), ("puts", 2), ("__stack_chk_fail", 1)]);
        let ag = compare(&f[&0x401150].calls, &out, &a2n, 0);
        assert_eq!(
            ag,
            Agreement {
                binary_calls: 4,
                output_calls: 4,
                matched: 3,
                missing: 1,
                excess: 1,
                exact: false
            }
        );
        assert!(compare(&f[&0x401136].calls, &calls(&[("puts", 1)]), &a2n, 0).exact);
    }
}

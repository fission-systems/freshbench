//! Recompilation: does the decompiled function compile back to the original code?
//!
//! For one function:
//! 1. The output is compiled **exactly as the decompiler printed it**, with the
//!    build's own compiler, target and optimization flags plus one fixed set of
//!    permissive C flags (`-c -w -fpermissive -std=gnu17 -fno-builtin`), the same
//!    for every tool. Nothing is added to or rewritten in the text: no prelude of
//!    pseudo-types, no declarations injected from compiler diagnostics, no
//!    repair loop. A decompiler whose output needs a type or declaration it
//!    did not print does not compile, and that is the measurement: such
//!    "fixups" score the fixer, and a fixer written around one decompiler's
//!    dialect favours that decompiler.
//! 2. The original function's bytes `[low_pc, high_pc)` and the recompiled
//!    function are disassembled with `llvm-objdump` and compared instruction
//!    by instruction with edit distance. Every constant, displacement and
//!    register must be equal. A branch inside the function is compared by
//!    its offset from the function's start. Only an instruction that carries
//!    a **relocation** in the recompiled object may differ, and only in the
//!    operand the linker fills in (`same_insn`): the object does not know
//!    that address yet, the linked original does.
//!
//! Scores: `compiles`, `similarity` (1 - edit distance / longer length, over
//! normalized instructions) and `exact` (identical sequences). A function that
//! does not compile has no similarity. This is recompilation equivalence of
//! code shape, not of meaning.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct Toolchain {
    /// compiler command up to (not including) flags and files, e.g. ["clang", "--target=..."]
    pub cc: Vec<String>,
    /// environment for the compiler (the build image's fbcc wrapper reads FB_ARCH/FB_FLAGS)
    pub env: Vec<(String, String)>,
    /// the build's optimization flags, e.g. ["-O2", "-fno-inline"]
    pub flags: Vec<String>,
    pub objdump: PathBuf,
    /// x86_64, i686, aarch64 or armv7 (disassembly syntax)
    pub arch: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecompileScore {
    pub compiles: bool,
    pub similarity: Option<f64>,
    pub exact: bool,
    pub original_insns: u32,
    pub recompiled_insns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn compile(tc: &Toolchain, src: &Path, obj: &Path) -> Result<(bool, String)> {
    let mut c = Command::new(&tc.cc[0]);
    c.envs(tc.env.iter().map(|(k, v)| (k, v)));
    c.args(&tc.cc[1..]).args(&tc.flags).args([
        "-c",
        "-w",
        "-fpermissive",
        "-std=gnu17",
        "-fno-builtin",
    ]);
    c.arg(src).arg("-o").arg(obj);
    let o = c.output().with_context(|| format!("run {}", tc.cc[0]))?;
    Ok((
        o.status.success(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    ))
}

/// One disassembled instruction (or literal-pool word).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Insn {
    /// mnemonic and operands as printed; constants kept; a code address with a
    /// `<sym+off>` annotation becomes `@+off` when it lies inside the function
    /// (so a branch is compared by where it goes) and `ADDR` otherwise
    pub text: String,
    /// the object file has a relocation in this instruction: its address
    /// operand is not known until link time
    pub reloc: bool,
    /// Symbol annotation of an out-of-function direct control transfer.
    pub external_target: Option<String>,
    /// Full relocation records, retaining type, symbol and addend.
    pub relocations: Vec<String>,
}

fn comment_start(arch: &str) -> &'static str {
    match arch {
        "aarch64" => "//",
        "armv7" => " @", // `#` is the immediate marker on ARM, not a comment
        _ => "#",        // x86 AT&T: immediates are `$n`
    }
}

/// Instructions from `llvm-objdump -d [-r] --no-show-raw-insn` text. Nothing a
/// compiler chose is dropped: immediates, displacements and registers stay.
pub fn parse_disassembly(text: &str, arch: &str) -> Vec<Insn> {
    let mut raw: Vec<(u64, String, Vec<String>)> = Vec::new();
    for line in text.lines() {
        let Some((addr, rest)) = line.split_once(':') else {
            continue;
        };
        let a = addr.trim();
        if a.is_empty() || !a.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let rest = rest.trim();
        if rest.starts_with("R_") {
            // a relocation line belongs to the instruction before it
            if let Some(last) = raw.last_mut() {
                last.2
                    .push(rest.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            continue;
        }
        if rest.is_empty() || rest.starts_with('<') {
            continue;
        }
        let mut s = rest.to_string();
        if let Some(i) = s.find(comment_start(arch)) {
            s.truncate(i);
        }
        let Ok(addr) = u64::from_str_radix(a, 16) else {
            continue;
        };
        raw.push((
            addr,
            s.split_whitespace().collect::<Vec<_>>().join(" "),
            Vec::new(),
        ));
    }
    let (Some(first), Some(last)) = (raw.first().map(|r| r.0), raw.last().map(|r| r.0)) else {
        return vec![];
    };
    raw.into_iter()
        .map(|(_, s, relocations)| {
            let external_target = external_target(&s, first, last);
            Insn {
                text: relative_targets(&s, first, last),
                reloc: !relocations.is_empty(),
                external_target,
                relocations,
            }
        })
        .collect()
}

/// Identify direct branch/call annotations outside the function. Do not treat
/// an address inside the current function as a symbol reference.
fn external_target(s: &str, first: u64, last: u64) -> Option<String> {
    let (before, annotation) = s.split_once('<')?;
    let symbol = annotation.split_once('>')?.0;
    let address = before.split_whitespace().last()?.strip_prefix("0x")?;
    let address = u64::from_str_radix(address, 16).ok()?;
    (address < first || address > last).then(|| symbol.to_string())
}

fn control_transfer(text: &str) -> bool {
    let mnemonic = text.split_whitespace().next().unwrap_or("");
    mnemonic.starts_with("call")
        || mnemonic.starts_with('j')
        || matches!(
            mnemonic,
            "b" | "bl" | "blx" | "b.w" | "b.n" | "cbz" | "cbnz" | "tbz" | "tbnz"
        )
        || mnemonic.starts_with("b.")
        || matches!(
            mnemonic,
            "beq"
                | "bne"
                | "bgt"
                | "bge"
                | "blt"
                | "ble"
                | "bhi"
                | "bls"
                | "bcc"
                | "bcs"
                | "bmi"
                | "bpl"
                | "bvs"
                | "bvc"
        )
}

/// Accept only known direct-control relocation forms for linked originals.
/// x86 PC-relative calls use the conventional -4 addend; ARM direct call
/// relocations normally have no printed addend. Other forms are not proof.
fn direct_relocation_target(record: &str) -> Option<&str> {
    let (kind, target) = record.split_once(' ')?;
    match kind {
        "R_X86_64_PLT32" | "R_X86_64_PC32" => target.strip_suffix("-0x4"),
        "R_386_PC32" | "R_386_PLT32" | "R_AARCH64_CALL26" | "R_AARCH64_JUMP26" | "R_ARM_CALL"
        | "R_ARM_JUMP24" | "R_ARM_THM_CALL" | "R_ARM_THM_JUMP24" => {
            (!target.contains('+') && !target.contains('-')).then_some(target)
        }
        _ => None,
    }
}

/// `0x401136 <f+0x10>` -> `@+0x10` for a target inside `[first, last]`, else `ADDR`.
fn relative_targets(s: &str, first: u64, last: u64) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        let Some(gt) = rest[lt..].find('>') else {
            break;
        };
        let before = rest[..lt].trim_end();
        let tok_start = before
            .rfind(|c: char| !(c.is_ascii_alphanumeric()))
            .map_or(0, |i| i + 1);
        let tok = &before[tok_start..];
        match u64::from_str_radix(tok.trim_start_matches("0x"), 16) {
            Ok(t) if tok.starts_with("0x") => {
                out.push_str(&before[..tok_start]);
                if t >= first && t <= last {
                    out.push_str(&format!("@+{:#x}", t - first));
                } else {
                    out.push_str("ADDR");
                }
            }
            _ => out.push_str(before),
        }
        rest = &rest[lt + gt + 1..];
    }
    out.push_str(rest);
    out.trim_end().to_string()
}

/// `text` with its link-time operands replaced by `N`: address-like numbers
/// (`ADDR`, `@+off`, displacements, bare addresses) when `addresses`, else
/// `$n` / `#n` immediates. An elided zero displacement is the same as a
/// present one (`(%rip)` vs `0x2ecd(%rip)`, `[x8]` vs `[x8, #0xfc0]`).
fn mask(text: &str, addresses: bool) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if text[i..].starts_with("ADDR") || text[i..].starts_with("@+") {
            let mut j = i + if c == 'A' { 4 } else { 2 };
            while j < b.len() && (b[j] as char).is_ascii_alphanumeric() {
                j += 1;
            }
            out.push_str(if addresses { "N" } else { &text[i..j] });
            i = j;
            continue;
        }
        let prev = if i > 0 { b[i - 1] as char } else { ' ' };
        let starts_number = (c.is_ascii_digit()
            || (c == '-' && i + 1 < b.len() && (b[i + 1] as char).is_ascii_digit()))
            && !(prev.is_ascii_alphanumeric() || prev == '_' || prev == '%' || prev == '.');
        if starts_number {
            let mut j = i + usize::from(c == '-');
            while j < b.len() && (b[j] as char).is_ascii_alphanumeric() {
                j += 1;
            }
            let immediate = prev == '$' || prev == '#';
            out.push_str(if immediate != addresses {
                "N"
            } else {
                &text[i..j]
            });
            i = j;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out.replace("N(", "(").replace(", #N]", "]")
}

fn has_address_operand(text: &str) -> bool {
    mask(text, true) != text || text.contains("(%")
}

/// Are the original instruction `a` and the recompiled `b` the same
/// instruction? Identical text, or - only when `b` carries a relocation -
/// identical up to the operand the linker fills in: the address operand when
/// either has one, else the `$`/`#` immediate (`add x0, x0, :lo12:sym`).
pub fn same_insn(a: &Insn, b: &Insn) -> bool {
    // Never discard a direct control-transfer target merely because both
    // addresses were normalized to ADDR or both objects contain placeholders.
    if control_transfer(&a.text) || control_transfer(&b.text) {
        if a.reloc || b.reloc {
            if a.reloc {
                if a.relocations != b.relocations || !b.reloc {
                    return false;
                }
            } else {
                let Some(target) = a.external_target.as_deref() else {
                    return false;
                };
                let target = target.strip_suffix("@plt").unwrap_or(target);
                if b.relocations.len() != 1
                    || direct_relocation_target(&b.relocations[0]) != Some(target)
                {
                    return false;
                }
            }
        } else if a.external_target != b.external_target {
            return false;
        }
    }
    if a.text == b.text {
        return true;
    }
    if !b.reloc {
        return false;
    }
    let addresses = has_address_operand(&a.text) || has_address_operand(&b.text);
    mask(&a.text, addresses) == mask(&b.text, addresses)
}

pub fn edit_distance(a: &[Insn], b: &[Insn]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j + 1] + 1)
                .min(cur[j] + 1)
                .min(prev[j] + usize::from(!same_insn(x, y)));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn objdump(tc: &Toolchain, args: &[String], file: &Path) -> Result<String> {
    let o = Command::new(&tc.objdump)
        .args(["-d", "--no-show-raw-insn"])
        .args(args)
        .arg(file)
        .output()?;
    if !o.status.success() {
        bail!("llvm-objdump: {}", String::from_utf8_lossy(&o.stderr));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// (section name, start, end) of a defined function symbol in an object file;
/// the Thumb bit is not part of the address.
fn symbol_range(obj: &Path, name: &str) -> Result<(String, u64, u64)> {
    use object::{Object, ObjectSection, ObjectSymbol};
    let data = std::fs::read(obj)?;
    let f = object::File::parse(&*data)?;
    let arm = matches!(f.architecture(), object::Architecture::Arm);
    let sym = f
        .symbols()
        .find(|s| s.name() == Ok(name) && s.size() > 0)
        .with_context(|| format!("no symbol {name} in the recompiled object"))?;
    let section = match sym.section() {
        object::SymbolSection::Section(i) => f.section_by_index(i)?.name()?.to_string(),
        _ => bail!("symbol {name} is not in a section"),
    };
    let lo = if arm {
        sym.address() & !1
    } else {
        sym.address()
    };
    Ok((section, lo, lo + sym.size()))
}

/// The name the output defines (what the recompiled object's symbol is called).
fn defined_name(output: &str, wanted: &str) -> Option<String> {
    let t = crate::csyntax::parse(output).ok()?;
    let d = crate::csyntax::output_function(&t, output, wanted)?;
    crate::csyntax::function_name(d, output).map(str::to_string)
}

/// Recompile `output` (the decompiler's text for `function`) and compare it with the
/// original function `[low, high)` of the unstripped `binary`.
pub fn score(
    tc: &Toolchain,
    output: &str,
    function: &str,
    binary: &Path,
    low: u64,
    high: u64,
) -> RecompileScore {
    let mut r = RecompileScore::default();
    let Some(name) = defined_name(output, function) else {
        r.error = Some("no function definition in the output".into());
        return r;
    };
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    };
    let (src, obj) = (dir.path().join("f.c"), dir.path().join("f.o"));
    if std::fs::write(&src, output).is_err() {
        r.error = Some("write failed".into());
        return r;
    }
    match compile(tc, &src, &obj) {
        Ok((true, _)) => r.compiles = true,
        Ok((false, err)) => {
            r.error = Some(
                err.lines()
                    .find(|l| l.contains("error"))
                    .unwrap_or("compile failed")
                    .chars()
                    .take(300)
                    .collect(),
            );
            return r;
        }
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    }
    let original = objdump(
        tc,
        &[
            "-r".to_string(),
            format!("--start-address={low:#x}"),
            format!("--stop-address={high:#x}"),
        ],
        binary,
    )
    .map(|t| parse_disassembly(&t, &tc.arch));
    // the recompiled function is cut the same way as the original, by its
    // address range (symbol value and size), so a literal pool inside the
    // function is on both sides or on neither
    let recompiled = symbol_range(&obj, &name).and_then(|(section, lo, hi)| {
        objdump(
            tc,
            &[
                "-r".to_string(),
                format!("--section={section}"),
                format!("--start-address={lo:#x}"),
                format!("--stop-address={hi:#x}"),
            ],
            &obj,
        )
        .map(|t| parse_disassembly(&t, &tc.arch))
    });
    match (original, recompiled) {
        (Ok(a), Ok(b)) => {
            let d = edit_distance(&a, &b);
            let n = a.len().max(b.len()).max(1);
            r.similarity = Some(1.0 - d as f64 / n as f64);
            r.exact = !a.is_empty() && d == 0 && a.len() == b.len();
            r.original_insns = a.len() as u32;
            r.recompiled_insns = b.len() as u32;
        }
        (Err(e), _) | (_, Err(e)) => r.error = Some(e.to_string()),
    }
    r
}

/// The build image's toolchain for (arch, compiler): the fbcc wrapper that built the
/// corpus, so the recompile uses the same compiler, target and base flags.
pub fn image_toolchain(arch: &str, compiler: &str, flags: &str) -> Toolchain {
    Toolchain {
        cc: vec![format!("fbcc-{compiler}")],
        env: vec![
            ("FB_ARCH".into(), arch.into()),
            ("FB_FLAGS".into(), flags.into()),
        ],
        flags: vec![],
        objdump: PathBuf::from("/opt/llvm/bin/llvm-objdump"),
        arch: arch.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insns(text: &str, arch: &str) -> Vec<Insn> {
        parse_disassembly(text, arch)
    }

    #[test]
    fn constants_survive_and_only_relocations_are_wild() {
        // AArch64: `#` is an immediate, not a comment
        let a = insns(
            "0000000000000000 <f>:\n 0:\tmov\tw0, #0x1\n 4:\tret\n",
            "aarch64",
        );
        let b = insns(
            "0000000000000000 <f>:\n 0:\tmov\tw0, #0x2\n 4:\tret\n",
            "aarch64",
        );
        assert_eq!(a[0].text, "mov w0, #0x1");
        assert_eq!(edit_distance(&a, &b), 1);
        assert_eq!(edit_distance(&a, &a), 0);
        // x86: a hex immediate is a constant too
        let x = insns(" 0:\taddl\t$0x11, %eax\n", "x86_64");
        let y = insns(" 0:\taddl\t$0x12, %eax\n", "x86_64");
        assert!(!same_insn(&x[0], &y[0]));
    }

    #[test]
    fn external_calls_keep_symbol_identity() {
        let a = insns(" 1000: callq 0x2000 <foo@plt>\n", "x86_64");
        let b = insns(" 1000: callq 0x3000 <bar@plt>\n", "x86_64");
        assert_eq!(edit_distance(&a, &b), 1);
        let same = insns(
            " 0: callq 0x5 <f+0x5>\n 1: R_X86_64_PLT32 foo-0x4\n",
            "x86_64",
        );
        let wrong = insns(
            " 0: callq 0x5 <f+0x5>\n 1: R_X86_64_PLT32 bar-0x4\n",
            "x86_64",
        );
        assert_eq!(edit_distance(&a, &same), 0);
        assert_eq!(edit_distance(&a, &wrong), 1);
        assert_eq!(edit_distance(&same, &wrong), 1);
    }

    #[test]
    fn linked_original_against_relocatable_object() {
        // the linked original: resolved call and GOT load, a branch inside f
        let orig = insns(
            "0000000000401130 <f>:\n\
  401130:\tcmpl\t$0x4, %edi\n\
  401133:\tjl\t0x40114b <f+0x1b>\n\
  401135:\tcallq\t0x401030 <ext@plt>\n\
  40113a:\tmovq\t0x2e9f(%rip), %rcx            # 0x403fe0 <g>\n\
  401141:\tmovl\t$0x5, 0x2ec5(%rip)            # 0x404010 <h>\n\
  40114b:\tretq\n",
            "x86_64",
        );
        // the recompiled object: relocations, zero displacements
        let obj = "0000000000000000 <f>:\n\
       0:\tcmpl\t$0x4, %edi\n\
       3:\tjl\t0x1b <f+0x1b>\n\
       5:\tcallq\t0xa <f+0xa>\n\
\t\t0000000000000006:  R_X86_64_PLT32\text-0x4\n\
       a:\tmovq\t(%rip), %rcx            # 0x11 <f+0x11>\n\
\t\t000000000000000d:  R_X86_64_REX_GOTPCRELX\tg-0x4\n\
      11:\tmovl\t$0x5, (%rip)            # 0x1b <f+0x1b>\n\
\t\t0000000000000013:  R_X86_64_PC32\th-0x8\n\
      1b:\tretq\n";
        let rec = insns(obj, "x86_64");
        assert_eq!(rec.iter().filter(|i| i.reloc).count(), 3);
        assert_eq!(orig[1].text, "jl @+0x1b");
        assert_eq!(edit_distance(&orig, &rec), 0);
        // the stored constant is still compared on a relocated instruction
        let wrong = insns(&obj.replace("$0x5, (%rip)", "$0x6, (%rip)"), "x86_64");
        assert_eq!(edit_distance(&orig, &wrong), 1);
        // a branch to another place in the function is a different instruction
        let moved = insns(
            &obj.replace("jl\t0x1b <f+0x1b>", "jl\t0x11 <f+0x11>"),
            "x86_64",
        );
        assert_eq!(edit_distance(&orig, &moved), 1);
        // without a relocation nothing is wild
        let no_reloc = insns(
            &obj.replace("\t\t0000000000000006:  R_X86_64_PLT32\text-0x4\n", ""),
            "x86_64",
        );
        assert_eq!(no_reloc.iter().filter(|i| i.reloc).count(), 2);
        assert_eq!(edit_distance(&orig, &no_reloc), 1);
    }

    #[test]
    fn aarch64_page_and_lo12() {
        let orig = insns(
            "  400580:\tadrp\tx8, 0x411000 <g>\n  400584:\tldr\tx8, [x8, #0xfc0]\n  400588:\tadd\tw0, w8, #0x2\n  40058c:\tbl\t0x400460 <ext@plt>\n",
            "aarch64",
        );
        let obj =
            "       0:\tadrp\tx8, 0x0 <f>\n\t\t0000000000000000:  R_AARCH64_ADR_GOT_PAGE\tg\n\
       4:\tldr\tx8, [x8]\n\t\t0000000000000004:  R_AARCH64_LD64_GOT_LO12_NC\tg\n\
       8:\tadd\tw0, w8, #0x2\n\
       c:\tbl\t0xc <f+0xc>\n\t\t000000000000000c:  R_AARCH64_CALL26\text\n";
        assert_eq!(edit_distance(&orig, &insns(obj, "aarch64")), 0);
        let wrong = insns(
            &obj.replace("add\tw0, w8, #0x2", "add\tw0, w8, #0x3"),
            "aarch64",
        );
        assert_eq!(edit_distance(&orig, &wrong), 1);
        // ARM: `@` starts the comment
        let arm = insns(
            "       c:\tblt\t0x28 <f+0x28>           @ imm = #0x14\n      10:\tadd\tr0, r0, #17\n",
            "armv7",
        );
        assert_eq!(arm[1].text, "add r0, r0, #17");
        assert!(
            !arm[0].text.contains('@') || arm[0].text.contains("@+") || arm[0].text == "blt ADDR"
        );
    }

    /// Needs an LLVM with an x86-64 ELF target (Homebrew's or the build image's).
    #[test]
    fn recompile_roundtrip_when_llvm_present() {
        let llvm = [
            "/opt/llvm/bin",
            "/opt/homebrew/opt/llvm/bin",
            "/usr/lib/llvm-18/bin",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.join("clang").exists() && p.join("llvm-objdump").exists());
        let Some(llvm) = llvm else { return };
        let tc = Toolchain {
            cc: vec![
                llvm.join("clang").display().to_string(),
                "--target=x86_64-linux-gnu".into(),
            ],
            env: vec![],
            flags: vec!["-O2".into()],
            objdump: llvm.join("llvm-objdump"),
            arch: "x86_64".into(),
        };
        let d = tempfile::tempdir().unwrap();
        // the "original": compile a source function to an object, use it as the binary
        let orig_src = d.path().join("o.c");
        std::fs::write(&orig_src, "int f(int a){ return a * 3 + 1; }\n").unwrap();
        let orig = d.path().join("o.o");
        assert!(Command::new(&tc.cc[0])
            .args(&tc.cc[1..])
            .args(["-O2", "-c"])
            .arg(&orig_src)
            .arg("-o")
            .arg(&orig)
            .status()
            .unwrap()
            .success());
        let data = std::fs::read(&orig).unwrap();
        let f = object::File::parse(&*data).unwrap();
        use object::{Object, ObjectSymbol};
        let sym = f.symbols().find(|s| s.name() == Ok("f")).unwrap();
        let (lo, hi) = (sym.address(), sym.address() + sym.size());
        let same = score(
            &tc,
            "int FUN_00001000(int param_1)\n{\n  return param_1 * 3 + 1;\n}\n",
            "f",
            &orig,
            lo,
            hi,
        );
        assert!(same.compiles && same.exact, "{same:?}");
        // a type the output uses but never defines is not supplied: no fixups
        for undefined in [
            "undefined4 f(uint a)\n{\n  return a * 3 + 1;\n}\n",
            "int f(int a)\n{\n  Foo x = 0;\n  return a * 3 + 1 + (int)x;\n}\n",
        ] {
            let r = score(&tc, undefined, "f", &orig, lo, hi);
            assert!(!r.compiles && r.error.is_some(), "{r:?}");
        }
        // what the decompiler defines itself is its own business
        let own = score(
            &tc,
            "typedef unsigned int uint;\nuint f(uint a)\n{\n  return a * 3 + 1;\n}\n",
            "f",
            &orig,
            lo,
            hi,
        );
        assert!(own.compiles, "{own:?}");
        let other = score(
            &tc,
            "int f(int a)\n{\n  return a * 7 - 2;\n}\n",
            "f",
            &orig,
            lo,
            hi,
        );
        assert!(other.compiles && !other.exact && other.similarity.unwrap() < 1.0);
        let broken = score(
            &tc,
            "int f(int a)\n{\n  return a->b;\n}\n",
            "f",
            &orig,
            lo,
            hi,
        );
        assert!(!broken.compiles && broken.error.is_some());
    }

    /// The review's case on every M1 ISA: a function recompiled with another
    /// constant is not instruction-identical, and one that calls and loads a
    /// global (relocations) is identical to itself.
    #[test]
    fn a_wrong_constant_is_never_exact_on_any_isa() {
        let Some(llvm) = ["/opt/llvm/bin", "/opt/homebrew/opt/llvm/bin"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.join("clang").exists() && p.join("llvm-objdump").exists())
        else {
            return;
        };
        let src = |k: u32| {
            format!(
                "extern int ext(int); extern int g;\nint f(int a)\n{{\n  if (a > 3) g = ext(a) + 17;\n  return a * 3 + {k};\n}}\n"
            )
        };
        for (arch, target) in [
            ("x86_64", "x86_64-linux-gnu"),
            ("i686", "i686-linux-gnu"),
            ("aarch64", "aarch64-linux-gnu"),
            ("armv7", "armv7-linux-gnueabihf"),
        ] {
            let tc = Toolchain {
                cc: vec![
                    llvm.join("clang").display().to_string(),
                    format!("--target={target}"),
                ],
                env: vec![],
                flags: vec!["-O1".into(), "-fPIC".into()],
                objdump: llvm.join("llvm-objdump"),
                arch: arch.into(),
            };
            let d = tempfile::tempdir().unwrap();
            let (c, o) = (d.path().join("o.c"), d.path().join("o.o"));
            std::fs::write(&c, src(1)).unwrap();
            assert!(Command::new(&tc.cc[0])
                .args(&tc.cc[1..])
                .args(["-O1", "-fPIC", "-c"])
                .arg(&c)
                .arg("-o")
                .arg(&o)
                .status()
                .unwrap()
                .success());
            let data = std::fs::read(&o).unwrap();
            let f = object::File::parse(&*data).unwrap();
            use object::{Object, ObjectSymbol};
            let sym = f.symbols().find(|s| s.name() == Ok("f")).unwrap();
            let (lo, hi) = (sym.address() & !1, (sym.address() & !1) + sym.size());
            let same = score(&tc, &src(1), "f", &o, lo, hi);
            assert!(same.compiles && same.exact, "{arch}: {same:?}");
            let wrong = score(&tc, &src(2), "f", &o, lo, hi);
            assert!(wrong.compiles && !wrong.exact, "{arch}: {wrong:?}");
            assert!(wrong.similarity.unwrap() < 1.0, "{arch}: {wrong:?}");
        }
    }
}

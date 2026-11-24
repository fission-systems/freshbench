//! Recompilation: does the decompiled function compile back to the original code?
//!
//! For one function:
//! 1. The output is prefixed with a prelude that defines the type names
//!    decompilers print (`undefined4`, `uint`, `byte`, ...), and compiled with the
//!    build's own compiler, target and optimization flags (`-c`, permissive C so
//!    implicit declarations are allowed).
//! 2. When the compiler reports an unknown type name or an undeclared
//!    identifier, a declaration is added (`typedef long T;` / `extern long X;`)
//!    and it is compiled again, at most [`MAX_FIXUPS`] times.
//! 3. The original function's bytes `[low_pc, high_pc)` and the recompiled
//!    function are disassembled with `llvm-objdump`; operands that are
//!    addresses or relocations are normalized away; the two instruction
//!    sequences are compared with edit distance.
//!
//! Scores: `compiles`, `fixups`, `similarity` (1 - edit distance / longer
//! length, over normalized instructions) and `exact` (identical sequences).
//! A function that does not compile has no similarity. This is recompilation
//! equivalence of code shape, not of meaning.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const MAX_FIXUPS: u32 = 5;

/// Type names decompilers print, defined for the target's data model.
pub fn prelude(long_bytes: u8) -> String {
    let l = if long_bytes == 8 { "long" } else { "long long" };
    let mut p = String::from(
        "typedef unsigned char uchar; typedef unsigned short ushort; typedef unsigned int uint;\n\
typedef unsigned long ulong; typedef long long longlong; typedef unsigned long long ulonglong;\n\
typedef unsigned char byte; typedef unsigned short word; typedef unsigned int dword;\n\
typedef unsigned char undefined; typedef unsigned char undefined1; typedef unsigned short undefined2;\n\
typedef unsigned int undefined3; typedef unsigned int undefined4; typedef unsigned long long undefined5;\n\
typedef unsigned long long undefined6; typedef unsigned long long undefined7; typedef unsigned long long undefined8;\n\
typedef float float4; typedef double float8; typedef long double float10;\n\
typedef _Bool bool; typedef unsigned char uint8_t; typedef unsigned short uint16_t; typedef unsigned int uint32_t;\n\
typedef unsigned long long uint64_t; typedef signed char int8_t; typedef short int16_t; typedef int int32_t;\n\
typedef long long int64_t; typedef unsigned long size_t; typedef long ssize_t; typedef unsigned char u8;\n\
typedef unsigned short u16; typedef unsigned int u32; typedef unsigned long long u64;\n",
    );
    p += &format!("typedef unsigned {l} qword; typedef {l} code;\n");
    p += "#define true 1\n#define false 0\n";
    p
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    /// compiler command up to (not including) flags and files, e.g. ["clang", "--target=..."]
    pub cc: Vec<String>,
    /// environment for the compiler (the build image's fbcc wrapper reads FB_ARCH/FB_FLAGS)
    pub env: Vec<(String, String)>,
    /// the build's optimization flags, e.g. ["-O2", "-fno-inline"]
    pub flags: Vec<String>,
    pub objdump: PathBuf,
    pub long_bytes: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RecompileScore {
    pub compiles: bool,
    pub fixups: u32,
    pub similarity: Option<f64>,
    pub exact: bool,
    pub original_insns: u32,
    pub recompiled_insns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Is `name` used as a type in `source`: `Name ident`, `Name *ident`, `(Name)` cast?
fn used_as_type(source: &str, name: &str) -> bool {
    let toks: Vec<&str> = source
        .split(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '_' || c == '*' || c == '(' || c == ')')
        })
        .filter(|t| !t.is_empty())
        .collect();
    let bare = |t: &str| {
        t.trim_matches(|c| c == '*' || c == '(' || c == ')')
            .to_string()
    };
    for (i, t) in toks.iter().enumerate() {
        if bare(t) != name {
            continue;
        }
        if t.starts_with('(') && t.ends_with(')') {
            return true; // (Name)x
        }
        if let Some(n) = toks.get(i + 1) {
            let n = n.trim_start_matches(['*', '(']);
            if n.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                return true; // Name x / Name *x
            }
        }
    }
    false
}

/// Declarations to add for the compiler's complaints; empty when none applies.
/// Type names come first: when a round has any, only those are added, because
/// the identifiers they declare are reported as undeclared too.
pub fn fixups_from_diagnostics(stderr: &str, source: &str) -> Vec<String> {
    let (mut types, mut vars) = (Vec::new(), Vec::new());
    for line in stderr.lines() {
        let quoted = |l: &str| -> Option<String> {
            let s = l.find(['\'', '\u{2018}'])?;
            let rest = &l[s + l[s..].chars().next()?.len_utf8()..];
            let e = rest.find(['\'', '\u{2019}'])?;
            Some(rest[..e].to_string())
        };
        let ident_ok =
            |n: &str| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if line.contains("unknown type name") {
            if let Some(n) = quoted(line).filter(|n| ident_ok(n)) {
                types.push(format!("typedef long {n};"));
            }
        } else if line.contains("undeclared") {
            if let Some(n) = quoted(line).filter(|n| ident_ok(n)) {
                if used_as_type(source, &n) {
                    types.push(format!("typedef long {n};"));
                } else {
                    vars.push(format!("extern long {n};"));
                }
            }
        }
    }
    let mut out = if types.is_empty() { vars } else { types };
    out.sort();
    out.dedup();
    out
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

/// Normalized instruction lines from `llvm-objdump -d --no-show-raw-insn` text:
/// mnemonic and operands, with addresses (`0x...`, `<sym+off>`, `# comments`) removed.
pub fn normalize_disassembly(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((addr, rest)) = line.split_once(':') else {
            continue;
        };
        if addr.trim().is_empty() || !addr.trim().chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let mut s = rest.trim().to_string();
        if s.is_empty() || s.starts_with('<') {
            continue;
        }
        if let Some(i) = s.find(['#', ';']) {
            if s.starts_with("j") || s.starts_with("call") || s.starts_with('b') || i > 0 {
                s.truncate(i);
            }
        }
        if let Some(i) = s.find('<') {
            s.truncate(i);
        }
        let toks: Vec<String> = s
            .split_whitespace()
            .map(|t| {
                let t = t.trim_end_matches(',');
                let hexlike = t.trim_start_matches(['$', '#', '-']).starts_with("0x");
                if hexlike {
                    "ADDR".to_string()
                } else {
                    t.to_string()
                }
            })
            .collect();
        if !toks.is_empty() {
            out.push(toks.join(" "));
        }
    }
    out
}

pub fn edit_distance(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j + 1] + 1)
                .min(cur[j] + 1)
                .min(prev[j] + usize::from(x != y));
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
    let mut extra: Vec<String> = Vec::new();
    loop {
        let text = format!(
            "{}{}\n{}\n",
            prelude(tc.long_bytes),
            extra.join("\n"),
            output
        );
        if std::fs::write(&src, text).is_err() {
            r.error = Some("write failed".into());
            return r;
        }
        match compile(tc, &src, &obj) {
            Ok((true, _)) => {
                r.compiles = true;
                break;
            }
            Ok((false, err)) => {
                let add: Vec<String> = fixups_from_diagnostics(&err, output)
                    .into_iter()
                    .filter(|d| !extra.contains(d))
                    .collect();
                if add.is_empty() || r.fixups >= MAX_FIXUPS {
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
                r.fixups += 1;
                extra.extend(add);
            }
            Err(e) => {
                r.error = Some(e.to_string());
                return r;
            }
        }
    }
    let original = objdump(
        tc,
        &[
            format!("--start-address={low:#x}"),
            format!("--stop-address={high:#x}"),
        ],
        binary,
    )
    .map(|t| normalize_disassembly(&t));
    let recompiled = objdump(tc, &[format!("--disassemble-symbols={name}")], &obj)
        .map(|t| normalize_disassembly(&t));
    match (original, recompiled) {
        (Ok(a), Ok(b)) => {
            let d = edit_distance(&a, &b);
            let n = a.len().max(b.len()).max(1);
            r.similarity = Some(1.0 - d as f64 / n as f64);
            r.exact = !a.is_empty() && a == b;
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
        long_bytes: crate::types::DataModel::for_arch(arch).long,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_to_fixups() {
        let gcc = "f.c:3:5: error: unknown type name 'Foo'\nf.c:4:9: error: 'DAT_00401000' undeclared (first use in this function)\n";
        assert_eq!(
            fixups_from_diagnostics(gcc, ""),
            ["typedef long Foo;"],
            "types first"
        );
        assert_eq!(
            fixups_from_diagnostics(
                "f.c:4:9: error: 'DAT_00401000' undeclared\n",
                "x = DAT_00401000;"
            ),
            ["extern long DAT_00401000;"]
        );
        let clang = "f.c:4:9: error: use of undeclared identifier 'Bar'\nf.c:5:1: error: use of undeclared identifier 'y'\n";
        assert_eq!(
            fixups_from_diagnostics(clang, "  Bar *y = 0;\n  return (int)y;"),
            ["typedef long Bar;"]
        );
        assert_eq!(
            fixups_from_diagnostics("error: use of undeclared identifier 'g'\n", "return g + 1;"),
            ["extern long g;"]
        );
        assert!(fixups_from_diagnostics("f.c: error: expected ';'\n", "").is_empty());
    }

    #[test]
    fn normalization_and_distance() {
        let t = "0000000000000000 <f>:\n       0:      \tleal\t(%rdi,%rdi,2), %eax\n       3:      \tcallq\t0x401020 <puts@plt>\n       8:      \tjmp\t0x10 <f+0x10>\n";
        assert_eq!(
            normalize_disassembly(t),
            ["leal (%rdi,%rdi,2) %eax", "callq ADDR", "jmp ADDR"]
        );
        let a: Vec<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let b: Vec<String> = ["x", "z"].iter().map(|s| s.to_string()).collect();
        assert_eq!(edit_distance(&a, &b), 1);
        assert_eq!(edit_distance(&a, &a), 0);
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
            long_bytes: 8,
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
        // a decompiler's spelling with its own types and an undeclared global it never uses
        let same = score(
            &tc,
            "undefined4 FUN_00001000(uint param_1)\n{\n  return param_1 * 3 + 1;\n}\n",
            "f",
            &orig,
            lo,
            hi,
        );
        assert!(same.compiles && same.exact, "{same:?}");
        let fixed = score(
            &tc,
            "int f(int a)\n{\n  Foo x = 0;\n  return a * 3 + 1 + (int)x;\n}\n",
            "f",
            &orig,
            lo,
            hi,
        );
        assert!(fixed.compiles && fixed.fixups == 1, "{fixed:?}");
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
}

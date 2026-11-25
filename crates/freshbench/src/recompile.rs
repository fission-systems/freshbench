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
//!    function are disassembled with `llvm-objdump`; operands that are
//!    addresses or relocations are normalized away; the two instruction
//!    sequences are compared with edit distance.
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

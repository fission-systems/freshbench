//! Function boundaries: can a tool find the functions of a stripped binary?
//!
//! The decompilation metrics hand every tool the function starts. This track
//! does not: the tool lists the functions its own analysis found
//! (`adapters/README.md`, list mode) and the list is compared with the
//! unstripped binary.
//!
//! Ground truth, per binary: every `STT_FUNC` symbol with a size whose start
//! lies in an executable section that has bytes in the file, and every DWARF
//! subprogram with a `[low_pc, high_pc)` range there. Addresses with the same
//! start are one function (aliases); DWARF's end wins over the symbol size.
//! Functions with DWARF are the project's own (`project`); the rest are
//! runtime and linker code (`_start`, `frame_dummy`, ...), counted separately
//! because they are the same in every binary.
//!
//! Scores: a found start equal to a true start is a start match (precision
//! over found, recall over truth). A start match whose end equals the true end
//! is an exact boundary; ending anywhere in `[true end, next true start]`
//! (alignment padding included) is a padded boundary. On ARM the Thumb bit is
//! cleared on both sides.
//!
//! Found starts inside PLT sections (`.plt`, `.plt.sec`, `.plt.got`, `.iplt`)
//! are not scored, for every tool alike: the truth has no functions there, and
//! some tools list PLT stubs while others filter them, which is a reporting
//! convention, not a boundary someone found or missed.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use object::{Object, ObjectSection, ObjectSymbol, SectionKind, SymbolKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrueFunction {
    pub start: u64,
    pub end: u64,
    pub project: bool,
}

fn exec_ranges(f: &object::File) -> Vec<(u64, u64)> {
    f.sections()
        .filter(|s| s.kind() == SectionKind::Text && s.file_range().is_some() && s.size() > 0)
        .map(|s| (s.address(), s.address() + s.size()))
        .collect()
}

/// Address ranges of the PLT sections, where found starts are not scored.
pub fn plt_ranges(data: &[u8]) -> Result<Vec<(u64, u64)>> {
    let f = object::File::parse(data)?;
    Ok(f.sections()
        .filter(|s| {
            matches!(
                s.name(),
                Ok(".plt" | ".plt.sec" | ".plt.got" | ".iplt" | ".plt.bnd")
            )
        })
        .map(|s| (s.address(), s.address() + s.size()))
        .collect())
}

/// The functions of an unstripped binary, sorted by start.
pub fn truth(data: &[u8]) -> Result<Vec<TrueFunction>> {
    let f = object::File::parse(data)?;
    let arm = matches!(f.architecture(), object::Architecture::Arm);
    let clear = |a: u64| if arm { a & !1 } else { a };
    let exec = exec_ranges(&f);
    let inside = |a: u64| exec.iter().any(|&(lo, hi)| a >= lo && a < hi);
    let mut out: BTreeMap<u64, TrueFunction> = BTreeMap::new();
    for s in f.symbols() {
        if s.kind() != SymbolKind::Text || s.size() == 0 || s.is_undefined() {
            continue;
        }
        let start = clear(s.address());
        if !inside(start) {
            continue;
        }
        out.entry(start).or_insert(TrueFunction {
            start,
            end: start + s.size(),
            project: false,
        });
    }
    for (lo, hi) in crate::dwarf::ranges(data)? {
        let start = clear(lo);
        if !inside(start) || hi <= lo {
            continue;
        }
        out.insert(
            start,
            TrueFunction {
                start,
                end: clear(hi).max(start + 1),
                project: true,
            },
        );
    }
    Ok(out.into_values().collect())
}

/// Sections that give function boundaries away on a stripped binary: unwind
/// tables (`.eh_frame` lists every function's exact range on x86-64 and
/// AArch64 Linux by default) and ARM's exception index.
pub const UNWIND_SECTIONS: [&str; 4] = [".eh_frame", ".eh_frame_hdr", ".ARM.exidx", ".ARM.extab"];

/// A copy of a little-endian ELF without its unwind information: the bytes of
/// [`UNWIND_SECTIONS`] are zeroed, their section headers become `SHT_NULL`,
/// and `PT_GNU_EH_FRAME` / `PT_ARM_EXIDX` become `PT_NULL`. Sizes and
/// addresses do not change; the result is for static analysis only.
/// Returns the copy and how many sections were neutralized.
pub fn without_unwind(data: &[u8]) -> Result<(Vec<u8>, usize)> {
    let f = object::File::parse(data)?;
    anyhow::ensure!(f.is_little_endian(), "big-endian ELF");
    let is64 = f.is_64();
    let mut out = data.to_vec();
    let rd = |o: usize, n: usize| -> u64 {
        let mut v = 0u64;
        for i in 0..n {
            v |= (data[o + i] as u64) << (8 * i);
        }
        v
    };
    let (shoff, shentsize, phoff, phentsize, phnum) = if is64 {
        (
            rd(0x28, 8) as usize,
            rd(0x3a, 2) as usize,
            rd(0x20, 8) as usize,
            rd(0x36, 2) as usize,
            rd(0x38, 2) as usize,
        )
    } else {
        (
            rd(0x20, 4) as usize,
            rd(0x2e, 2) as usize,
            rd(0x1c, 4) as usize,
            rd(0x2a, 2) as usize,
            rd(0x2c, 2) as usize,
        )
    };
    let mut n = 0;
    for s in f.sections() {
        let Ok(name) = s.name() else { continue };
        if !UNWIND_SECTIONS.contains(&name) {
            continue;
        }
        if let Some((off, len)) = s.file_range() {
            out[off as usize..(off + len) as usize].fill(0);
        }
        // sh_type is the second word of a section header (offset 4 in both classes)
        let h = shoff + s.index().0 * shentsize + 4;
        out[h..h + 4].fill(0);
        n += 1;
    }
    for i in 0..phnum {
        let h = phoff + i * phentsize;
        let t = rd(h, 4);
        if t == 0x6474_e550 || t == 0x7000_0001 {
            out[h..h + 4].fill(0);
        }
    }
    Ok((out, n))
}

/// One binary's boundary scores for one tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub truth: u64,
    pub truth_project: u64,
    pub found: u64,
    pub start_match: u64,
    pub start_match_project: u64,
    pub exact: u64,
    pub exact_project: u64,
    pub padded: u64,
    pub padded_project: u64,
}

impl std::ops::AddAssign<&Counts> for Counts {
    fn add_assign(&mut self, o: &Counts) {
        self.truth += o.truth;
        self.truth_project += o.truth_project;
        self.found += o.found;
        self.start_match += o.start_match;
        self.start_match_project += o.start_match_project;
        self.exact += o.exact;
        self.exact_project += o.exact_project;
        self.padded += o.padded;
        self.padded_project += o.padded_project;
    }
}

/// Compare what a tool found (`(start, end)`, end optional) with the truth.
pub fn score(
    truth: &[TrueFunction],
    found: &[(u64, Option<u64>)],
    arm: bool,
    unscored: &[(u64, u64)],
) -> Counts {
    let clear = |a: u64| if arm { a & !1 } else { a };
    let mut found_by_start: BTreeMap<u64, Option<u64>> = BTreeMap::new();
    for &(s, e) in found {
        if unscored
            .iter()
            .any(|&(lo, hi)| clear(s) >= lo && clear(s) < hi)
        {
            continue;
        }
        // a start listed twice counts once; keep an end if any listing had one
        let slot = found_by_start.entry(clear(s)).or_insert(None);
        if slot.is_none() {
            *slot = e.map(clear);
        }
    }
    let starts: BTreeSet<u64> = truth.iter().map(|t| t.start).collect();
    let mut c = Counts {
        truth: truth.len() as u64,
        truth_project: truth.iter().filter(|t| t.project).count() as u64,
        found: found_by_start.len() as u64,
        ..Default::default()
    };
    for t in truth {
        let Some(end) = found_by_start.get(&t.start) else {
            continue;
        };
        let p = u64::from(t.project);
        c.start_match += 1;
        c.start_match_project += p;
        if let Some(e) = *end {
            let next = starts
                .range(t.start + 1..)
                .next()
                .copied()
                .unwrap_or(u64::MAX);
            if e == t.end {
                c.exact += 1;
                c.exact_project += p;
            }
            if e >= t.end && e <= next.max(t.end) {
                c.padded += 1;
                c.padded_project += p;
            }
        }
    }
    c
}

/// One row of the boundary track: a (tool, binary) and its counts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub tool: String,
    /// `as-built` (the stripped binary as shipped) or `no-unwind`
    /// ([`without_unwind`])
    pub variant: String,
    pub project: String,
    pub arch: String,
    pub compiler: String,
    pub config: String,
    pub binary: String,
    /// identical stripped binaries share this (deduplication)
    pub stripped_sha256: String,
    #[serde(flatten)]
    pub counts: Counts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// wall time, CPU time and peak memory of the tool's run on this binary
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::adapters::Usage>,
    /// binaries the harness ran at once
    #[serde(default)]
    pub jobs: u32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Rates {
    pub binaries: u64,
    pub failed_binaries: u64,
    #[serde(flatten)]
    pub counts: Counts,
    pub start_precision: f64,
    pub start_recall: f64,
    pub start_f1: f64,
    pub project_start_recall: f64,
    pub exact_rate: f64,
    pub project_exact_rate: f64,
    pub padded_rate: f64,
}

fn div(a: u64, b: u64) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64
    }
}

impl Rates {
    fn finish(mut self) -> Rates {
        let c = &self.counts;
        self.start_precision = div(c.start_match, c.found);
        self.start_recall = div(c.start_match, c.truth);
        let (p, r) = (self.start_precision, self.start_recall);
        self.start_f1 = if p + r > 0.0 {
            2.0 * p * r / (p + r)
        } else {
            0.0
        };
        self.project_start_recall = div(c.start_match_project, c.truth_project);
        self.exact_rate = div(c.exact, c.truth);
        self.project_exact_rate = div(c.exact_project, c.truth_project);
        self.padded_rate = div(c.padded, c.truth);
        self
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    /// "tool variant" -> all binaries
    pub overall: BTreeMap<String, Rates>,
    /// tool -> identical stripped binaries counted once
    pub distinct_binaries: BTreeMap<String, Rates>,
    /// dimension (arch, compiler, config) -> value -> tool
    pub by: BTreeMap<String, BTreeMap<String, BTreeMap<String, Rates>>>,
}

/// Micro-averaged rates per tool, overall, over distinct binaries, and by
/// arch / compiler / level. A binary the tool failed on keeps its truth in the
/// denominators (found 0).
pub fn summarize(rows: &[Row]) -> Summary {
    let mut s = Summary::default();
    let add = |r: &mut Rates, row: &Row| {
        r.binaries += 1;
        r.failed_binaries += u64::from(row.error.is_some());
        r.counts += &row.counts;
    };
    let mut seen: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
    for row in rows {
        let tv = format!("{} {}", row.tool, row.variant);
        add(s.overall.entry(tv.clone()).or_default(), row);
        let k = (
            row.tool.as_str(),
            row.variant.as_str(),
            row.stripped_sha256.as_str(),
        );
        if seen.insert(k) {
            add(s.distinct_binaries.entry(tv.clone()).or_default(), row);
        }
        for (dim, val) in [
            ("arch", &row.arch),
            ("compiler", &row.compiler),
            ("config", &row.config),
        ] {
            add(
                s.by.entry(dim.into())
                    .or_default()
                    .entry(val.clone())
                    .or_default()
                    .entry(tv.clone())
                    .or_default(),
                row,
            );
        }
    }
    let fin = |m: &mut BTreeMap<String, Rates>| {
        for v in m.values_mut() {
            *v = std::mem::take(v).finish();
        }
    };
    fin(&mut s.overall);
    fin(&mut s.distinct_binaries);
    for d in s.by.values_mut() {
        for m in d.values_mut() {
            fin(m);
        }
    }
    s
}

/// Boundary rows from files or directories of `*.jsonl` (other lines skipped).
pub fn load(paths: &[impl AsRef<std::path::Path>]) -> Result<Vec<Row>> {
    let mut files = Vec::new();
    for p in paths {
        let p = p.as_ref();
        if p.is_dir() {
            crate::rows::walk_sorted(p, &mut |f| {
                if f.extension().is_some_and(|e| e == "jsonl") {
                    files.push(f.to_path_buf())
                }
            })?;
        } else {
            files.push(p.to_path_buf());
        }
    }
    let mut out = Vec::new();
    for f in files {
        for line in std::fs::read_to_string(&f)?.lines() {
            if let Ok(r) = serde_json::from_str::<Row>(line) {
                out.push(r);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(start: u64, end: u64, project: bool) -> TrueFunction {
        TrueFunction {
            start,
            end,
            project,
        }
    }

    #[test]
    fn starts_ends_and_padding() {
        let truth = [
            tf(0x1000, 0x1010, false),
            tf(0x1020, 0x1050, true),
            tf(0x1060, 0x1070, true),
        ];
        let found = [
            (0x1000, Some(0x1010)), // exact
            (0x1020, Some(0x1060)), // ends in the padding up to the next start
            (0x1060, None),         // start only
            (0x1080, Some(0x1090)), // not a function
            (0x1020, None),         // listed twice
        ];
        let c = score(&truth, &found, false, &[]);
        assert_eq!(
            c,
            Counts {
                truth: 3,
                truth_project: 2,
                found: 4,
                start_match: 3,
                start_match_project: 2,
                exact: 1,
                exact_project: 0,
                padded: 2,
                padded_project: 1,
            }
        );
        // a PLT stub is neither found nor missed
        let plt = score(
            &truth,
            &[(0x1000, None), (0x500, None)],
            false,
            &[(0x400, 0x600)],
        );
        assert_eq!((plt.found, plt.start_match), (1, 1));
        // past the next function's start is not a boundary
        let c = score(&truth, &[(0x1020, Some(0x1068))], false, &[]);
        assert_eq!((c.start_match, c.padded), (1, 0));
    }

    /// Needs an LLVM clang with ELF targets (Homebrew's or the build image's).
    #[test]
    fn unwind_tables_are_removed_when_clang_present() {
        use std::process::Command;
        let Some(clang) = ["/opt/llvm/bin/clang", "/opt/homebrew/opt/llvm/bin/clang"]
            .iter()
            .map(std::path::PathBuf::from)
            .find(|p| p.exists())
        else {
            return;
        };
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.c");
        std::fs::write(
            &src,
            "int g(int x){return x*3;}\nint f(int x){return g(x)+1;}\n",
        )
        .unwrap();
        for (target, sections) in [
            ("x86_64-linux-gnu", &[".eh_frame"][..]),
            ("armv7-linux-gnueabihf", &[".ARM.exidx"][..]),
        ] {
            let obj = d.path().join(format!("{target}.o"));
            assert!(Command::new(&clang)
                .args([&format!("--target={target}"), "-O1", "-c"])
                .arg(&src)
                .arg("-o")
                .arg(&obj)
                .status()
                .unwrap()
                .success());
            let data = std::fs::read(&obj).unwrap();
            let before = object::File::parse(&*data).unwrap();
            for s in sections {
                let sec = before.section_by_name(s).expect(s);
                assert!(sec.data().unwrap().iter().any(|&b| b != 0), "{target} {s}");
            }
            let (out, n) = without_unwind(&data).unwrap();
            assert!(n >= sections.len(), "{target}: {n}");
            assert_eq!(out.len(), data.len());
            let after = object::File::parse(&*out).unwrap();
            for s in sections {
                // the header is SHT_NULL and nothing readable is left
                let left = after
                    .section_by_name(s)
                    .and_then(|x| x.data().ok().map(|d| d.iter().any(|&b| b != 0)))
                    .unwrap_or(false);
                assert!(!left, "{target} {s} still has unwind bytes");
            }
            // the code itself is untouched
            let text =
                |f: &object::File| f.section_by_name(".text").unwrap().data().unwrap().to_vec();
            assert_eq!(text(&before), text(&after));
        }
    }

    #[test]
    fn thumb_bit_is_cleared() {
        let truth = [tf(0x1000, 0x1010, true)];
        let c = score(&truth, &[(0x1001, Some(0x1010))], true, &[]);
        assert_eq!((c.start_match, c.exact), (1, 1));
        // not on x86: 0x1001 is another address
        assert_eq!(score(&truth, &[(0x1001, None)], false, &[]).start_match, 0);
    }

    #[test]
    fn summary_keeps_failed_binaries_and_dedups() {
        let row = |bin: &str, sha: &str, c: Counts, err: Option<&str>| Row {
            tool: "t".into(),
            variant: "as-built".into(),
            project: "p".into(),
            arch: "x86_64".into(),
            compiler: "gcc".into(),
            config: "O2".into(),
            binary: bin.into(),
            stripped_sha256: sha.into(),
            counts: c,
            error: err.map(Into::into),
            usage: None,
            jobs: 1,
        };
        let good = Counts {
            truth: 10,
            truth_project: 8,
            found: 10,
            start_match: 9,
            start_match_project: 8,
            ..Default::default()
        };
        let failed = Counts {
            truth: 10,
            truth_project: 8,
            ..Default::default()
        };
        let rows = [
            row("a", "s1", good.clone(), None),
            row("b", "s1", good.clone(), None), // the same binary built twice
            row("c", "s2", failed, Some("crash")),
        ];
        let s = summarize(&rows);
        let o = &s.overall["t as-built"];
        assert_eq!((o.binaries, o.failed_binaries), (3, 1));
        assert_eq!(o.start_recall, 18.0 / 30.0);
        assert_eq!(o.start_precision, 18.0 / 20.0);
        let d = &s.distinct_binaries["t as-built"];
        assert_eq!((d.binaries, d.start_recall), (2, 9.0 / 20.0));
    }

    /// Linux only: a real ELF's symbols and DWARF agree with the truth.
    #[cfg(target_os = "linux")]
    #[test]
    fn truth_from_elf() {
        use std::process::Command;
        if Command::new("cc").arg("--version").output().is_err() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.c");
        std::fs::write(
            &src,
            "static int g(int x){return x*3;}\nint main(int c, char **v){return g(c);}\n",
        )
        .unwrap();
        let bin = d.path().join("a.out");
        assert!(Command::new("cc")
            .args(["-g", "-O0"])
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .status()
            .unwrap()
            .success());
        let t = truth(&std::fs::read(&bin).unwrap()).unwrap();
        assert_eq!(t.iter().filter(|f| f.project).count(), 2);
        assert!(
            t.iter().any(|f| !f.project),
            "runtime code (_start) is listed too"
        );
        assert!(t.windows(2).all(|w| w[0].start < w[1].start));
    }
}

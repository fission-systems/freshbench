//! Scoring defects DecBench found over the years, kept as tests of FreshBench.
//!
//! FreshBench shares no code with DecBench, but a defect in how a benchmark
//! scores is a defect in the idea, and a new implementation can make it again.
//! One test per DecBench issue or pull request that has a counterpart here;
//! `docs/decbench-compatibility-audit.md` lists them all, including the ones
//! that are not solved.

use std::path::PathBuf;

use freshbench::cfg::{self, Graph};
use freshbench::csyntax;
use freshbench::types::{self, DataModel, TypeClass};

fn graph(src: &str, name: &str) -> Graph {
    let tree = csyntax::parse(src).unwrap();
    let defs = csyntax::definitions(&tree, src);
    cfg::build(defs[name].expect("one definition"), src)
}

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// #5: a timeout kills the tool's whole process group, not only the child.
#[cfg(unix)]
#[test]
fn decbench_005_timeout_leaves_no_orphans() {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    let d = tempfile::tempdir().unwrap();
    let pidf = d.path().join("pid");
    let mut c = Command::new("sh");
    c.arg("-c")
        .arg(format!("sleep 30 & echo $! > {}; wait", pidf.display()));
    let (ok, _, err) =
        freshbench::adapters::run_with_timeout(c, Duration::from_millis(500)).unwrap();
    assert!(!ok && err.starts_with("timeout"));
    std::thread::sleep(Duration::from_millis(200));
    let pid = std::fs::read_to_string(&pidf).unwrap();
    let alive = Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive);
}

/// #6: a source without control flow has a degenerate CFG that an empty or
/// truncated output matches. FreshBench keeps such rows but reports them in
/// their own band and outside "perfect on functions with control flow".
#[test]
fn decbench_006_degenerate_source_cfg_is_not_evidence() {
    let src = "int a(int); int b(int);\n\
               int full(int x) { x = a(x); x = b(x); return x + 1; }\n\
               int truncated(int x) { }\n\
               int branchy(int x) { if (x) x = a(x); return x; }\n";
    // the defect's precondition is real here too: the two are the same graph
    assert_eq!(graph(src, "full"), graph(src, "truncated"));
    assert_eq!(graph(src, "full").nodes(), 2);
    // so the band, not the score, has to carry the information
    assert_eq!(
        freshbench::bands::band(Some(graph(src, "full").nodes() as u32)),
        freshbench::bands::STRAIGHT_LINE
    );
    assert_ne!(
        freshbench::bands::band(Some(graph(src, "branchy").nodes() as u32)),
        freshbench::bands::STRAIGHT_LINE
    );
}

/// #8: a tool with no result for a row keeps the row in its denominator.
#[test]
fn decbench_008_missing_results_stay_in_the_denominator() {
    let row = |f: &str, tool: &str, ged: f64| {
        freshbench::rows::parse_row(&format!(
            r#"{{"project":"p","arch":"x86_64","compiler":"gcc","config":"O0","binary":"b","function":"{f}","tool":"{tool}","outcome":"scored","scorer":"s","ged":{{"value":{ged},"exact":true}}}}"#
        ))
        .unwrap()
    };
    let mut t = freshbench::rows::Table::new();
    for r in [row("f", "a", 0.0), row("g", "a", 5.0), row("f", "b", 0.0)] {
        t.entry(r.key.clone())
            .or_default()
            .insert(r.tool.clone(), r);
    }
    let s = freshbench::report::summarize(&t, 20);
    let b = &s.overall["b"];
    assert_eq!((b.rows, b.missing, b.perfect), (2, 1, 1));
    assert_eq!(b.perfect_rate, 0.5);
}

/// #57 (and #52): two large isomorphic CFGs score 0 whatever the GED budget.
#[test]
fn decbench_057_large_isomorphic_cfgs_are_perfect() {
    use freshbench::cfg::Label;
    let n = 300u32;
    let mut edges = Vec::new();
    for i in 0..n - 1 {
        edges.push((i, i + 1));
        if i % 4 == 0 && i + 9 < n {
            edges.push((i, i + 9));
        }
        if i % 13 == 7 {
            edges.push((i, i - 5));
        }
    }
    edges.sort_unstable();
    let mut labels = vec![Label::Block; n as usize];
    labels[0] = Label::Entry;
    labels[n as usize - 1] = Label::Exit;
    let a = Graph {
        labels: labels.clone(),
        edges: edges.clone(),
    };
    let perm: Vec<u32> = (0..n).map(|i| (i * 101 + 17) % n).collect();
    let mut pl = vec![Label::Block; n as usize];
    for (old, &new) in perm.iter().enumerate() {
        pl[new as usize] = labels[old];
    }
    let mut pe: Vec<_> = edges
        .iter()
        .map(|&(x, y)| (perm[x as usize], perm[y as usize]))
        .collect();
    pe.sort_unstable();
    let b = Graph {
        labels: pl,
        edges: pe,
    };
    let d = freshbench::ged::distance_with_budget(&a, &b, 1);
    assert_eq!((d.value, d.exact), (0, true));
}

/// #104: ARM32 (and i686) are ILP32; `long` and pointers are 4 bytes there.
#[test]
fn decbench_104_ilp32_targets_are_not_lp64() {
    for arch in ["armv7", "i686"] {
        let dm = DataModel::for_arch(arch);
        assert_eq!(
            types::class_of_name("long", dm),
            TypeClass::Int(4),
            "{arch}"
        );
        assert_eq!(
            types::class_of_name("unsigned long", dm),
            TypeClass::Int(4),
            "{arch}"
        );
        assert_eq!(
            types::class_of_name("size_t", dm),
            TypeClass::Int(4),
            "{arch}"
        );
    }
    let lp64 = DataModel::for_arch("aarch64");
    assert_eq!(types::class_of_name("long", lp64), TypeClass::Int(8));
}

/// #105: the spellings of an endless loop are one CFG.
#[test]
fn decbench_105_constant_true_loops_are_one_shape() {
    let src = "int a(int);\n\
               int w(int x) { while (1) { x = a(x); if (x) break; } return x; }\n\
               int f(int x) { for (;;) { x = a(x); if (x) break; } return x; }\n\
               int d(int x) { do { x = a(x); if (x) break; } while (1); return x; }\n";
    assert_eq!(graph(src, "w"), graph(src, "f"));
    assert_eq!(graph(src, "w"), graph(src, "d"));
}

/// #112: `restrict` and `_Atomic` do not hide the type under them.
#[test]
fn decbench_112_qualifiers_are_read_through() {
    let src =
        "int f(const char *restrict p, _Atomic int n, volatile long *const q) { return 0; }\n";
    let tree = csyntax::parse(src).unwrap();
    let defs = csyntax::definitions(&tree, src);
    let sig = types::from_source(defs["f"].unwrap(), src, DataModel::for_arch("x86_64"));
    assert_eq!(
        sig.params,
        [TypeClass::Ptr, TypeClass::Int(4), TypeClass::Ptr]
    );
}

/// #119 / #101: two functions with one name are never silently merged; the
/// name maps to nothing rather than to one of them.
#[test]
fn decbench_119_same_named_functions_are_not_merged() {
    let src = "static int helper(int x) { return x; }\n\
               int other(int x) { return x + 1; }\n\
               static int helper(int x) { if (x) return 1; return 2; }\n";
    let tree = csyntax::parse(src).unwrap();
    let defs = csyntax::definitions(&tree, src);
    assert!(defs["helper"].is_none());
    assert!(defs["other"].is_some());
}

/// #120: DWARF references into another compilation unit (`DW_FORM_ref_addr`,
/// GCC LTO) are followed; a type that cannot be read is unknown, never void.
#[test]
fn decbench_120_cross_unit_dwarf_references() {
    let sigs = freshbench::dwarf::signatures(&fixture("dwarf/gcc16_lto_aarch64.elf")).unwrap();
    // `double scale(const struct pt *, unsigned char)` with one int local
    let scale = sigs
        .values()
        .find(|s| s.ret == Some(TypeClass::Float(8)))
        .expect("scale: before the fix every type in it read as void");
    let mut params = scale.params.clone();
    params.sort();
    let mut want = vec![TypeClass::Ptr, TypeClass::Int(1)];
    want.sort();
    assert_eq!(params, want);
    assert_eq!(scale.locals, [TypeClass::Int(4)]);
    // nothing in the file reads as an all-void function with parameters
    assert!(!sigs
        .values()
        .any(|s| !s.params.is_empty() && s.params.iter().all(|p| *p == TypeClass::Void)));
}

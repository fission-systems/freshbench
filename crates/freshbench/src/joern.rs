//! The second CFG family: Joern-compatible CFGs from the vendored rust-joern
//! (Apache-2.0, `crates/rust-joern`), scored with FreshBench's own GED engine.
//!
//! rust-joern builds Joern's basic-block CFG (the graph DecBench-style GED is
//! defined on) for source translation units and decompiler output. The GED
//! itself is FreshBench's (`ged.rs`): same costs, same deterministic search.
//! So this family answers "how close is the output to the source in Joern's
//! CFG shape", next to FreshBench's own CFG definition (`cfg.rs`).

use std::collections::BTreeMap;

use rust_joern::graph::{Cfg, Options};

use crate::cfg::{Graph, Label};

/// Decompiler output cleanup before parsing, ported from rust-joern's Python
/// `_preprocess_decompilation` (itself a port of PyJoern's decompilation option):
/// spell MSVC-style integer and calling-convention keywords as C, and keep
/// C++ keywords `new`/`delete` in the first lines from breaking a C parse.
pub fn preprocess_decompilation(source: &str) -> String {
    let mut s = String::with_capacity(source.len());
    // `undefined [N] __rustcall` -> `undefined`
    let mut rest = source;
    while let Some(i) = rest.find("undefined") {
        s.push_str(&rest[..i]);
        let tail = &rest[i + "undefined".len()..];
        let t = tail.trim_start();
        if let Some(after_bracket) = t.strip_prefix('[') {
            if let Some(close) = after_bracket.find(']') {
                let digits = &after_bracket[..close];
                let after = after_bracket[close + 1..].trim_start();
                if (1..=4).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
                    if let Some(after_rc) = after.strip_prefix("__rustcall") {
                        s.push_str("undefined");
                        rest = after_rc;
                        continue;
                    }
                }
            }
        }
        s.push_str("undefined");
        rest = tail;
    }
    s.push_str(rest);
    for (old, new) in [
        ("__int8", "char"),
        ("__int16", "short"),
        ("__int32", "int"),
        ("__int64", "long"),
        ("__fastcall", ""),
        ("__noreturn", ""),
        ("__cdecl", ""),
        ("__rustcall", ""),
    ] {
        s = s.replace(old, new);
    }
    let mut lines: Vec<String> = s.split('\n').map(str::to_string).collect();
    for line in lines.iter_mut().take(3) {
        for (old, new) in [("new", "new_joern_token"), ("delete", "delete_joern_token")] {
            if line.contains(old) {
                *line = line.replace(old, new);
            }
        }
    }
    lines.join("\n")
}

/// Joern CFG -> FreshBench graph: entry and exit blocks labelled, ids renumbered
/// in node order, edges sorted and deduplicated. Joern's CFG has no separate
/// exit node (return blocks are leaves); `is_exitpoint` marks a block that is
/// both entry and exit of a straight-line function.
pub fn to_graph(cfg: &Cfg) -> Graph {
    let mut idx = BTreeMap::new();
    let mut labels = Vec::with_capacity(cfg.nodes.len());
    for (i, b) in cfg.nodes.iter().enumerate() {
        idx.insert(b.id, i as u32);
        labels.push(if b.is_entrypoint {
            Label::Entry
        } else if b.is_exitpoint {
            Label::Exit
        } else {
            Label::Block
        });
    }
    let mut edges: Vec<(u32, u32)> = cfg
        .edges
        .iter()
        .filter_map(|[a, b]| Some((*idx.get(a)?, *idx.get(b)?)))
        .collect();
    edges.sort_unstable();
    edges.dedup();
    Graph { labels, edges }
}

fn options(preprocessed: bool) -> Options {
    Options {
        language: Some("c".into()),
        preprocessed,
        ..Default::default()
    }
}

/// Joern CFG of every function in a preprocessed translation unit, by name.
/// A name defined more than once maps to None.
pub fn source_cfgs(tu: &str) -> BTreeMap<String, Option<Graph>> {
    let mut out: BTreeMap<String, Option<Graph>> = BTreeMap::new();
    let Ok(a) = rust_joern::analyze(tu, "source.i", &options(true)) else {
        return out;
    };
    for f in &a.functions {
        out.entry(f.name.clone())
            .and_modify(|e| *e = None)
            .or_insert_with(|| Some(to_graph(&f.cfg)));
    }
    out
}

/// Joern CFG of the function a decompiler printed: the one named `name`, else the only one.
pub fn output_cfg(text: &str, name: &str) -> Option<Graph> {
    let src = preprocess_decompilation(text);
    let a = rust_joern::analyze(&src, "output.c", &options(false)).ok()?;
    let named: Vec<_> = a.functions.iter().filter(|f| f.name == name).collect();
    let f = match named.as_slice() {
        [f] => *f,
        [] if a.functions.len() == 1 => &a.functions[0],
        _ => return None,
    };
    Some(to_graph(&f.cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess() {
        assert_eq!(
            preprocess_decompilation("undefined [8] __rustcall f(__int64 a)"),
            "undefined f(long a)"
        );
        assert_eq!(
            preprocess_decompilation("void __fastcall f(void)"),
            "void  f(void)"
        );
        assert_eq!(
            preprocess_decompilation("x\ny\nz\nint new;"),
            "x\ny\nz\nint new;"
        );
        assert_eq!(preprocess_decompilation("int new;"), "int new_joern_token;");
    }

    #[test]
    fn source_and_output_cfgs() {
        let tu =
            "int g(int);\nint f(int a, int b)\n{\n  if (a && b) return g(a);\n  return 0;\n}\n";
        let s = source_cfgs(tu);
        let sg = s["f"].clone().expect("f");
        // Joern's shape: an entry block, return blocks as leaves, no separate exit
        // node, and `a && b` inside one condition block
        assert_eq!(sg.labels.iter().filter(|l| **l == Label::Entry).count(), 1);
        assert_eq!((sg.nodes(), sg.edges.len()), (3, 2));
        // the same shape printed with other names: 0
        let same = output_cfg("int FUN_00401000(int param_1, int param_2)\n{\n  if (param_1 && param_2) return g(param_1);\n  return 0;\n}\n", "f")
            .expect("output");
        assert_eq!(crate::ged::distance(&sg, &same).value, 0);
        // Joern's CFG depends on how the condition is written: `x != 0 && y != 0`
        // is split into two condition blocks, `a && b` is not
        let split = output_cfg("int FUN_00401000(int param_1, int param_2)\n{\n  if (param_1 != 0 && param_2 != 0) {\n    return g(param_1);\n  }\n  return 0;\n}\n", "f")
            .expect("output");
        assert!(crate::ged::distance(&sg, &split).value > 0);
        assert!(output_cfg("garbage ;;", "f").is_none());
    }
}

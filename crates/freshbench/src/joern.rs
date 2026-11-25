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
use crate::csyntax;

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
    // parsed exactly as printed: no keyword rewriting or other dialect fixups
    // (the FreshBench CFG does none either)
    let a = rust_joern::analyze(text, "output.c", &options(false)).ok()?;
    // rust-joern also lists prototypes (`extern T free();`) as functions; the
    // output's functions are its definitions, the same set the FreshBench CFG
    // chooses from (`csyntax::output_function`), so a decompiler that prints
    // `sub_4108a4` next to its callees' prototypes is scored on both families
    let defined: std::collections::BTreeSet<String> = csyntax::parse(text)
        .map(|t| {
            csyntax::definitions(&t, text)
                .into_iter()
                .filter_map(|(n, d)| d.map(|_| n))
                .collect()
        })
        .unwrap_or_default();
    let defs: Vec<_> = a
        .functions
        .iter()
        .filter(|f| defined.contains(&f.name))
        .collect();
    let named: Vec<_> = defs.iter().filter(|f| f.name == name).collect();
    let f = match (named.as_slice(), defs.as_slice()) {
        ([f], _) => **f,
        ([], [only]) => *only,
        _ => return None,
    };
    Some(to_graph(&f.cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_function_beside_prototypes() {
        // a decompiler's own name, callee prototypes in both preprocessor arms
        let out = "#if defined(X)\nextern unsigned long long free(...);\n#else\nextern unsigned long long free();\n#endif\n\
                   void sub_4108a4(long *p)\n{\n    if (!p) {\n        return;\n    }\n    free(p);\n}\n";
        let g = output_cfg(out, "align_effect_destroy").expect("the only definition");
        assert_eq!(g, output_cfg(out, "sub_4108a4").unwrap());
        assert!(g.nodes() > 2);
    }

    #[test]
    fn source_cfgs_read_through_line_markers() {
        let tu = "# 1 \"a.c\"\nint f(int a)\n{\n int k[\n# 3 \"a.c\" 3\n 4\n# 3 \"a.c\"\n ];\n if (a) return 1;\n return 0;\n}\n";
        let plain = "int f(int a)\n{\n int k[\n 4\n ];\n if (a) return 1;\n return 0;\n}\n";
        let (m, p) = (source_cfgs(tu), source_cfgs(plain));
        let (m, p) = (m["f"].clone().expect("f"), p["f"].clone().expect("f"));
        assert_eq!(crate::ged::distance(&m, &p).value, 0);
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

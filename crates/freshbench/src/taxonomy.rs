//! Shape tags of one scored row: what the output prints compared with the source.
//!
//! Tags describe the text, not correctness. Node tags come from the two CFGs
//! (same builder); the rest from the same syntax counts on both sides.

use crate::csyntax::Features;

pub const TAGS: [&str; 9] = [
    "over_noded",
    "under_noded",
    "extra_goto",
    "extra_return",
    "lost_logical",
    "extra_ternary",
    "extra_calls",
    "missing_calls",
    "lost_switch",
];

pub fn tags(
    source_nodes: Option<u32>,
    output_nodes: Option<u32>,
    src: Option<&Features>,
    out: Option<&Features>,
) -> Vec<String> {
    let mut t: Vec<&str> = Vec::new();
    if let (Some(s), Some(o)) = (source_nodes, output_nodes) {
        if o > s {
            t.push("over_noded");
        } else if o < s {
            t.push("under_noded");
        }
    }
    if let (Some(s), Some(o)) = (src, out) {
        if o.goto > s.goto {
            t.push("extra_goto");
        }
        if o.returns > s.returns {
            t.push("extra_return");
        }
        if o.logical_and + o.logical_or < s.logical_and + s.logical_or {
            t.push("lost_logical");
        }
        if o.ternary > s.ternary {
            t.push("extra_ternary");
        }
        if o.n_calls() > s.n_calls() {
            t.push("extra_calls");
        } else if o.n_calls() < s.n_calls() {
            t.push("missing_calls");
        }
        if s.switches > 0 && o.switches == 0 {
            t.push("lost_switch");
        }
    }
    TAGS.iter()
        .filter(|x| t.contains(x))
        .map(|x| x.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csyntax::{features, output_function, parse};

    fn f(code: &str) -> Features {
        let t = parse(code).unwrap();
        features(output_function(&t, code, "f").unwrap(), code)
    }

    #[test]
    fn all_tags() {
        let src = f("int f(int a){ if (a && a > 2) return g(a); switch (a) { case 1: return 1; } return 0; }");
        let out = f("int f(int a){ if (a) { if (a > 2) goto L; } L: g(a); g(a); return a; return 0; return 1; return 2; x = a ? 1 : 2; }");
        assert_eq!(
            tags(Some(5), Some(7), Some(&src), Some(&out)),
            [
                "over_noded",
                "extra_goto",
                "extra_return",
                "lost_logical",
                "extra_ternary",
                "extra_calls",
                "lost_switch"
            ]
        );
        assert_eq!(tags(Some(9), Some(4), None, None), ["under_noded"]);
        assert!(tags(Some(4), Some(4), Some(&src), Some(&src)).is_empty());
    }
}

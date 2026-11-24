//! C syntax with tree-sitter-c: finding a function in a translation unit or in
//! a decompiler's output, and counting what its text prints.
//!
//! Source functions and decompiled functions go through the same parser and
//! the same counting, so a difference in a count is a difference in the text.
//! tree-sitter recovers from errors, so the non-standard C that decompilers
//! print (unknown type names, `undefined4`, odd casts) still yields statements.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use serde::Serialize;
use tree_sitter::{Node, Parser, Tree};

pub fn parse(text: &str) -> Result<Tree> {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_c::LANGUAGE.into())
        .map_err(|e| anyhow!("tree-sitter-c: {e}"))?;
    let blanked = blank_line_markers(text);
    p.parse(blanked.as_deref().unwrap_or(text), None)
        .ok_or_else(|| anyhow!("tree-sitter returned no tree"))
}

/// A preprocessor line marker: `# 51 "file.c" 3` or `#line 51 "file.c"`.
fn is_line_marker(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix('#') else {
        return false;
    };
    let rest = rest.trim_start();
    rest.starts_with(|c: char| c.is_ascii_digit())
        || rest
            .strip_prefix("line")
            .is_some_and(|r| r.starts_with([' ', '\t']))
}

/// The text with every line marker replaced by spaces of the same length, or
/// None when it has none. GCC's `-E` puts markers inside expressions wherever
/// a system-header macro expands (`key[\n# 51 "x.c" 3\n 32\n# 51 "x.c"\n]`);
/// tree-sitter-c accepts directives only between statements, so such a
/// function parsed as ERROR and its definition was not found. Markers carry
/// no C meaning, and blanking them in place keeps every byte offset, so nodes
/// still index the original text.
fn blank_line_markers(text: &str) -> Option<String> {
    if !text.lines().any(is_line_marker) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if is_line_marker(body) {
            out.extend(std::iter::repeat_n(' ', body.len()));
            out.push_str(&line[body.len()..]);
        } else {
            out.push_str(line);
        }
    }
    Some(out)
}

pub fn text<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

/// The declared name of a function_definition (through pointer/attributed declarators).
pub fn function_name<'a>(def: Node, src: &'a str) -> Option<&'a str> {
    let mut d = def.child_by_field_name("declarator")?;
    loop {
        match d.kind() {
            "function_declarator"
            | "pointer_declarator"
            | "parenthesized_declarator"
            | "attributed_declarator" => {
                d = d
                    .child_by_field_name("declarator")
                    .or_else(|| d.named_child(0))?;
            }
            "identifier" => return Some(text(d, src)),
            _ => return None,
        }
    }
}

/// Top-level function definitions: name -> node; a name defined twice maps to None.
pub fn definitions<'t>(tree: &'t Tree, src: &str) -> BTreeMap<String, Option<Node<'t>>> {
    let mut out: BTreeMap<String, Option<Node<'t>>> = BTreeMap::new();
    let root = tree.root_node();
    let mut cur = root.walk();
    for child in root.named_children(&mut cur) {
        collect_defs(child, src, &mut out);
    }
    out
}

fn collect_defs<'t>(n: Node<'t>, src: &str, out: &mut BTreeMap<String, Option<Node<'t>>>) {
    match n.kind() {
        "function_definition" => {
            if let Some(name) = function_name(n, src) {
                out.entry(name.to_string())
                    .and_modify(|e| *e = None)
                    .or_insert(Some(n));
            }
        }
        // `extern "C" { ... }`-like wrappers and preprocessor conditionals left in the
        // text; and ERROR: tree-sitter's error recovery can wrap a long stretch of a
        // TU, function definitions included, in one ERROR node
        "ERROR"
        | "linkage_specification"
        | "declaration_list"
        | "preproc_if"
        | "preproc_ifdef"
        | "preproc_else"
        | "preproc_elif" => {
            let mut c = n.walk();
            for ch in n.named_children(&mut c) {
                collect_defs(ch, src, out);
            }
        }
        _ => {}
    }
}

/// The function a decompiler printed for `name`: its definition if present,
/// else the only definition in the text. None when absent or ambiguous.
pub fn output_function<'t>(tree: &'t Tree, src: &str, name: &str) -> Option<Node<'t>> {
    let defs = definitions(tree, src);
    if let Some(found) = defs.get(name) {
        return *found;
    }
    let all: Vec<_> = defs.values().flatten().collect();
    if all.len() == 1 {
        Some(*all[0])
    } else {
        None
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Features {
    pub goto: u32,
    pub labels: u32,
    pub returns: u32,
    pub ifs: u32,
    pub loops: u32,
    pub switches: u32,
    pub cases: u32,
    pub logical_and: u32,
    pub logical_or: u32,
    pub ternary: u32,
    pub indirect_calls: u32,
    pub parse_errors: u32,
    /// Direct calls by callee name.
    pub calls: BTreeMap<String, u32>,
    /// Integer literals by value.
    #[serde(skip)]
    pub int_constants: BTreeMap<i128, u32>,
    #[serde(skip)]
    pub strings: BTreeMap<String, u32>,
}

impl Features {
    pub fn n_calls(&self) -> u32 {
        self.calls.values().sum()
    }
}

/// Counts over the subtree of `n` (normally a function_definition).
pub fn features(n: Node, src: &str) -> Features {
    let mut f = Features::default();
    let mut stack = vec![n];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "goto_statement" => f.goto += 1,
            "labeled_statement" => f.labels += 1,
            "return_statement" => f.returns += 1,
            "if_statement" => f.ifs += 1,
            "for_statement" | "while_statement" | "do_statement" => f.loops += 1,
            "switch_statement" => f.switches += 1,
            "case_statement" => f.cases += 1,
            "conditional_expression" => f.ternary += 1,
            "ERROR" => f.parse_errors += 1,
            "binary_expression" => match n.child_by_field_name("operator").map(|o| o.kind()) {
                Some("&&") => f.logical_and += 1,
                Some("||") => f.logical_or += 1,
                _ => {}
            },
            "call_expression" => match n.child_by_field_name("function") {
                Some(func) if func.kind() == "identifier" => {
                    *f.calls.entry(text(func, src).to_string()).or_default() += 1;
                }
                _ => f.indirect_calls += 1,
            },
            "number_literal" => {
                if let Some(v) = int_value(text(n, src)) {
                    let neg = n.parent().is_some_and(|p| {
                        p.kind() == "unary_expression"
                            && p.child_by_field_name("operator")
                                .is_some_and(|o| o.kind() == "-")
                    });
                    *f.int_constants.entry(if neg { -v } else { v }).or_default() += 1;
                }
            }
            "string_literal" => *f.strings.entry(text(n, src).to_string()).or_default() += 1,
            _ => {}
        }
        let mut c = n.walk();
        let kids: Vec<_> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    f
}

/// Value of an integer literal (suffixes ignored); None for floating point.
pub fn int_value(lit: &str) -> Option<i128> {
    let s = lit.trim_end_matches(['u', 'U', 'l', 'L']);
    let s = s.replace('\'', "");
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return i128::from_str_radix(h, 16).ok();
    }
    if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        return i128::from_str_radix(b, 2).ok();
    }
    if s.contains(['.', 'e', 'E', 'f', 'F', 'p', 'P']) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return i128::from_str_radix(&s[1..], 8).ok();
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feats(code: &str, name: &str) -> Features {
        let t = parse(code).unwrap();
        let def = output_function(&t, code, name).expect("function");
        features(def, code)
    }

    #[test]
    fn control_counts() {
        let src = r#"
int f(int a, int b) {
    if (a && b || !a) goto out;
    for (int i = 0; i < a; i++) { a += i; }
    while (b) b--;
    do { a++; } while (a < 10);
    switch (a) { case 1: return 1; case 2: break; default: return 0; }
    b = a ? 1 : 2;
out:
    return b;
}"#;
        let f = feats(src, "f");
        assert_eq!(
            (f.goto, f.labels, f.returns, f.ifs, f.loops, f.switches, f.cases),
            (1, 1, 3, 1, 3, 1, 3)
        );
        assert_eq!(
            (f.logical_and, f.logical_or, f.ternary, f.parse_errors),
            (1, 1, 1, 0)
        );
    }

    #[test]
    fn calls_and_comments() {
        let code = r#"extern long syscall(long n, ...);
/* goto x; foo(); */
unsigned g(long p) {
    x = (int)foo(p) + sizeof(int);
    s->cb(p);
    if (!bar(1)) return baz(2, qux(3)); // nope();
    return 0;
}"#;
        let f = feats(code, "g");
        let want: BTreeMap<String, u32> = [("bar", 1), ("baz", 1), ("foo", 1), ("qux", 1)]
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        assert_eq!(f.calls, want);
        assert_eq!(f.indirect_calls, 1);
        assert_eq!(f.goto, 0);
    }

    #[test]
    fn constants() {
        let f = feats(
            "void h(void){ x = 0x10 + 010 - 3; y = -7; z = 1.5; w = 42u; }",
            "h",
        );
        assert_eq!(f.int_constants.get(&16), Some(&1));
        assert_eq!(f.int_constants.get(&8), Some(&1));
        assert_eq!(f.int_constants.get(&3), Some(&1));
        assert_eq!(f.int_constants.get(&-7), Some(&1));
        assert_eq!(f.int_constants.get(&42), Some(&1));
        assert_eq!(int_value("0xffUL"), Some(255));
        assert_eq!(int_value("1e3"), None);
    }

    #[test]
    fn definitions_skip_prototypes_and_flag_duplicates() {
        let tu = "static int helper(int x);\nstatic int *helper2(void) { return 0; }\nstatic int\nhelper(int x)\n{ return x; }\nint dup(void){return 1;}\nint dup(void){return 2;}\n";
        let t = parse(tu).unwrap();
        let d = definitions(&t, tu);
        assert!(d["helper"].is_some());
        assert!(d["helper2"].is_some());
        assert!(d["dup"].is_none());
        assert!(!d.contains_key("nothere"));
    }

    /// GCC -E output: line markers inside an option-table initializer, where
    /// a system-header macro expanded. Shape found by minimizing a real GCC
    /// 16.2.0 TU whose definition tree-sitter-c lost; identifiers are made up.
    const GCC_MARKERS: &str = "int run(int n, char **v)\n{\n char buf[\n ];\n static struct opt table[] = {\n  {\"alpha\",\n ((void *)0)\n , 'a'},\n  {\"beta\",\n# 12 \"t.c\" 3\n   0\n# 12 \"t.c\"\n   , 'b'},\n  {\"gamma\",\n# 13 \"t.c\" 3\n   1\n# 13 \"t.c\"\n   , 'g'},\n  {\"delta\",\n   1\n   , 'd'},\n };\n while ((c = next(n, v, table)) != -1) {\n  switch (c) {\n  }\n }\n return finish(buf, c);\n}\n";

    #[test]
    fn line_markers_inside_expressions() {
        let plain: String = GCC_MARKERS
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| format!("{l}\n"))
            .collect();
        let graph = |src: &str| {
            let t = parse(src).unwrap();
            let d = definitions(&t, src);
            let def = d.get("run").copied().flatten().expect("cmd defined once");
            assert_eq!(
                text(def, src).lines().next(),
                Some("int run(int n, char **v)")
            );
            crate::cfg::build(def, src)
        };
        // the same function with and without markers: same graph
        assert_eq!(graph(GCC_MARKERS), graph(&plain));
        assert!(is_line_marker("#line 7 \"a.c\""));
        assert!(!is_line_marker("#define X 1"));
        assert!(!is_line_marker("#linex"));
        assert_eq!(blank_line_markers("int x;\n"), None);
    }

    #[test]
    fn definitions_inside_error_recovery() {
        // a real function (lumabri) whose `(typeof(*seen)){a, b}` compound literal makes
        // tree-sitter-c wrap the rest of the file, the next definition included, in ERROR
        let tu = include_str!("../tests/fixtures/typeof_compound_literal.c");
        let t = parse(tu).unwrap();
        let top = t.root_node();
        let mut c = top.walk();
        let has_error = top.children(&mut c).any(|n| n.kind() == "ERROR");
        assert!(has_error, "fixture must trigger error recovery");
        let d = definitions(&t, tu);
        assert!(
            d.get("after").is_some_and(|x| x.is_some()),
            "{:?}",
            d.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn decompiler_types_still_parse_statements() {
        let code = "undefined4 FUN_00401000(undefined8 param_1)\n{\n  uint uVar1;\n  uVar1 = func_0x00401200(param_1);\n  if (uVar1 == 0) { return 1; }\n  return 0;\n}\n";
        let t = parse(code).unwrap();
        let def = output_function(&t, code, "other_name").expect("only definition is taken");
        let f = features(def, code);
        assert_eq!((f.ifs, f.returns), (1, 2));
        assert_eq!(f.calls.get("func_0x00401200"), Some(&1));
    }
}

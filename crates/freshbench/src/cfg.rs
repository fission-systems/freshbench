//! Control-flow graphs of C functions, one builder for source and output.
//!
//! The source function (from the preprocessed translation unit) and the
//! decompiled function go through the same builder, so GED compares like with
//! like: two texts with the same control flow get the same graph, whatever
//! syntax they use for it.
//!
//! Definition (FreshBench's, documented here because the score depends on it):
//! * Nodes are basic blocks: maximal straight-line runs of statements, each
//!   ending in at most one branch. One ENTRY node (the first block) and one
//!   EXIT node (every `return`, every call to a no-return function, and the
//!   end of the body lead there).
//! * Short-circuit `&&`/`||` and `?:` are control flow, as in the machine
//!   code: `if (a && b)` and `if (a) if (b)` give the same graph.
//! * A condition that is an integer or boolean literal is folded (`while (1)`,
//!   `do { } while (0)`), as every compiler does.
//! * `switch` is one branch node with an edge to every distinct case target
//!   (and to the code after it when there is no `default`).
//! * Empty blocks are removed, straight-line chains are merged, nodes not
//!   reachable from ENTRY are dropped, and parallel edges count once.
//! * Calls are not nodes; `goto`, `break`, `continue` and `return;` are edges,
//!   not blocks. A computed `goto *p` ends the path with no known successor.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use tree_sitter::Node;

use crate::csyntax::text;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Label {
    Entry,
    Exit,
    Block,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Graph {
    pub labels: Vec<Label>,
    /// Sorted, unique (from, to).
    pub edges: Vec<(u32, u32)>,
}

impl Graph {
    pub fn nodes(&self) -> usize {
        self.labels.len()
    }
}

/// Calls that do not return; the path ends at EXIT.
pub const NORETURN: [&str; 18] = [
    "exit",
    "_exit",
    "_Exit",
    "quick_exit",
    "abort",
    "__assert_fail",
    "__assert_rtn",
    "__assert",
    "__stack_chk_fail",
    "longjmp",
    "siglongjmp",
    "_longjmp",
    "err",
    "errx",
    "verr",
    "verrx",
    "pthread_exit",
    "__builtin_unreachable",
];

const NOOP: &[&str] = &[
    "comment",
    ";",
    "{",
    "}",
    "type_definition",
    "struct_specifier",
    "enum_specifier",
    "union_specifier",
];

struct B<'s> {
    src: &'s str,
    /// per node: has a statement (non-empty)
    full: Vec<bool>,
    succ: Vec<BTreeSet<u32>>,
    labels: BTreeMap<String, u32>,
    breaks: Vec<u32>,
    continues: Vec<u32>,
    switches: Vec<(u32, bool)>, // (switch node, has default)
}

const ENTRY: u32 = 0;
const EXIT: u32 = 1;

impl<'s> B<'s> {
    fn new(src: &'s str) -> Self {
        B {
            src,
            full: vec![false, false],
            succ: vec![BTreeSet::new(), BTreeSet::new()],
            labels: BTreeMap::new(),
            breaks: vec![],
            continues: vec![],
            switches: vec![],
        }
    }

    fn node(&mut self, full: bool) -> u32 {
        self.full.push(full);
        self.succ.push(BTreeSet::new());
        (self.full.len() - 1) as u32
    }

    fn edge(&mut self, a: u32, b: u32) {
        self.succ[a as usize].insert(b);
    }

    /// A block holding one statement, entered from `cur` (unreachable if None).
    fn stmt_node(&mut self, cur: Option<u32>) -> u32 {
        let n = self.node(true);
        if let Some(c) = cur {
            self.edge(c, n);
        }
        n
    }

    fn label(&mut self, name: &str) -> u32 {
        if let Some(&n) = self.labels.get(name) {
            return n;
        }
        let n = self.node(false);
        self.labels.insert(name.to_string(), n);
        n
    }

    fn named_children(n: Node<'_>) -> Vec<Node<'_>> {
        let mut c = n.walk();
        n.named_children(&mut c).collect()
    }

    /// Statement; returns where control continues (None: it does not fall through).
    fn stmt(&mut self, n: Node, cur: Option<u32>) -> Option<u32> {
        match n.kind() {
            "compound_statement" => {
                let mut cur = cur;
                for ch in Self::named_children(n) {
                    cur = self.stmt(ch, cur);
                }
                cur
            }
            "if_statement" => {
                let c = n.child_by_field_name("condition");
                let then_n = self.node(false);
                let else_n = self.node(false);
                let after = self.node(false);
                match c {
                    Some(c) => self.cond(c, cur, then_n, else_n),
                    None => {
                        if let Some(x) = cur {
                            self.edge(x, then_n);
                        }
                    }
                }
                if let Some(t) = n.child_by_field_name("consequence") {
                    if let Some(e) = self.stmt(t, Some(then_n)) {
                        self.edge(e, after);
                    }
                } else {
                    self.edge(then_n, after);
                }
                match n.child_by_field_name("alternative") {
                    Some(alt) => {
                        let body = if alt.kind() == "else_clause" {
                            alt.named_child(0)
                        } else {
                            Some(alt)
                        };
                        match body.map(|b| self.stmt(b, Some(else_n))) {
                            Some(Some(e)) => self.edge(e, after),
                            Some(None) => {}
                            None => self.edge(else_n, after),
                        }
                    }
                    None => self.edge(else_n, after),
                }
                Some(after)
            }
            "while_statement" => {
                let header = self.node(false);
                let body = self.node(false);
                let after = self.node(false);
                if let Some(x) = cur {
                    self.edge(x, header);
                }
                match n.child_by_field_name("condition") {
                    Some(c) => self.cond(c, Some(header), body, after),
                    None => self.edge(header, body),
                }
                self.loop_body(n.child_by_field_name("body"), body, header, after);
                Some(after)
            }
            "do_statement" => {
                let body = self.node(false);
                let cond_n = self.node(false);
                let after = self.node(false);
                if let Some(x) = cur {
                    self.edge(x, body);
                }
                self.loop_body(n.child_by_field_name("body"), body, cond_n, after);
                match n.child_by_field_name("condition") {
                    Some(c) => self.cond(c, Some(cond_n), body, after),
                    None => self.edge(cond_n, after),
                }
                Some(after)
            }
            "for_statement" => {
                let mut cur = cur;
                if let Some(init) = n.child_by_field_name("initializer") {
                    cur = if init.kind() == "declaration" {
                        self.stmt(init, cur)
                    } else {
                        self.expr_stmt(init, cur)
                    };
                }
                let header = self.node(false);
                let body = self.node(false);
                let update = self.node(false);
                let after = self.node(false);
                if let Some(x) = cur {
                    self.edge(x, header);
                }
                match n.child_by_field_name("condition") {
                    Some(c) => self.cond(c, Some(header), body, after),
                    None => self.edge(header, body),
                }
                self.loop_body(n.child_by_field_name("body"), body, update, after);
                let mut u = Some(update);
                if let Some(upd) = n.child_by_field_name("update") {
                    u = self.expr_stmt(upd, u);
                }
                if let Some(u) = u {
                    self.edge(u, header);
                }
                Some(after)
            }
            "switch_statement" => {
                let mut cur = cur;
                if let Some(c) = n.child_by_field_name("condition") {
                    cur = self.expr(c, cur);
                }
                let sw = self.stmt_node(cur);
                let after = self.node(false);
                self.switches.push((sw, false));
                self.breaks.push(after);
                let end = n
                    .child_by_field_name("body")
                    .and_then(|b| self.stmt(b, None));
                self.breaks.pop();
                let (_, has_default) = self.switches.pop().unwrap();
                if let Some(e) = end {
                    self.edge(e, after);
                }
                if !has_default {
                    self.edge(sw, after);
                }
                Some(after)
            }
            "case_statement" => {
                let target = self.node(false);
                if let Some(x) = cur {
                    self.edge(x, target); // fall-through from the previous case
                }
                if let Some(top) = self.switches.last_mut() {
                    if n.child_by_field_name("value").is_none() {
                        top.1 = true;
                    }
                    let sw = top.0;
                    self.edge(sw, target);
                }
                let mut cur = Some(target);
                let value = n.child_by_field_name("value").map(|v| v.id());
                for ch in Self::named_children(n) {
                    if Some(ch.id()) == value {
                        continue;
                    }
                    cur = self.stmt(ch, cur);
                }
                cur
            }
            "labeled_statement" => {
                let name = n
                    .child_by_field_name("label")
                    .map(|l| text(l, self.src).to_string())
                    .unwrap_or_default();
                let l = self.label(&name);
                if let Some(x) = cur {
                    self.edge(x, l);
                }
                let label_id = n.child_by_field_name("label").map(|l| l.id());
                let mut cur = Some(l);
                for ch in Self::named_children(n) {
                    if Some(ch.id()) != label_id {
                        cur = self.stmt(ch, cur);
                    }
                }
                cur
            }
            "goto_statement" => {
                // a jump is an edge, not a block
                if let (Some(c), Some(l)) = (cur, n.child_by_field_name("label")) {
                    let t = self.label(text(l, self.src));
                    self.edge(c, t);
                }
                None // computed `goto *p` (no label field): no known successor
            }
            "return_statement" => match n.named_child(0) {
                Some(v) => {
                    let c = self.expr(v, cur);
                    let s = self.stmt_node(c);
                    self.edge(s, EXIT);
                    None
                }
                None => {
                    if let Some(c) = cur {
                        self.edge(c, EXIT);
                    }
                    None
                }
            },
            "break_statement" | "continue_statement" => {
                let t = if n.kind() == "break_statement" {
                    self.breaks.last()
                } else {
                    self.continues.last()
                };
                if let (Some(c), Some(&t)) = (cur, t) {
                    self.edge(c, t);
                }
                None
            }
            "expression_statement" => match n.named_child(0) {
                Some(e) => self.expr_stmt(e, cur),
                None => cur, // `;`
            },
            "declaration" => {
                // only initialized declarators are code
                let mut cur = cur;
                let mut any = false;
                for d in Self::named_children(n) {
                    if d.kind() == "init_declarator" {
                        if let Some(v) = d.child_by_field_name("value") {
                            any = true;
                            cur = self.expr_stmt(v, cur);
                        }
                    }
                }
                let _ = any;
                cur
            }
            k if NOOP.contains(&k) => cur,
            _ => {
                // anything else (ERROR, asm, attributes, unknown statements): one block
                Some(self.stmt_node(cur))
            }
        }
    }

    fn loop_body(&mut self, body: Option<Node>, entry: u32, cont: u32, after: u32) {
        self.breaks.push(after);
        self.continues.push(cont);
        let end = match body {
            Some(b) => self.stmt(b, Some(entry)),
            None => Some(entry),
        };
        self.breaks.pop();
        self.continues.pop();
        if let Some(e) = end {
            self.edge(e, cont);
        }
    }

    /// An expression evaluated for effect: one statement block plus any control flow inside it.
    fn expr_stmt(&mut self, e: Node, cur: Option<u32>) -> Option<u32> {
        let c = self.expr(e, cur);
        if self.is_noreturn_call(e) {
            let s = self.stmt_node(c);
            self.edge(s, EXIT);
            return None;
        }
        Some(self.stmt_node(c))
    }

    fn is_noreturn_call(&self, e: Node) -> bool {
        let e = strip_parens(e);
        e.kind() == "call_expression"
            && e.child_by_field_name("function")
                .is_some_and(|f| f.kind() == "identifier" && NORETURN.contains(&text(f, self.src)))
    }

    /// Control flow inside an expression (`&&`, `||`, `?:`); returns the join.
    fn expr(&mut self, e: Node, cur: Option<u32>) -> Option<u32> {
        match e.kind() {
            "binary_expression" if matches!(op(e), Some("&&") | Some("||")) => {
                let t = self.node(false);
                let f = self.node(false);
                let join = self.node(false);
                self.cond(e, cur, t, f);
                self.edge(t, join);
                self.edge(f, join);
                Some(join)
            }
            "conditional_expression" => {
                let t = self.node(false);
                let f = self.node(false);
                let join = self.node(false);
                match e.child_by_field_name("condition") {
                    Some(c) => self.cond(c, cur, t, f),
                    None => {
                        if let Some(x) = cur {
                            self.edge(x, t);
                        }
                    }
                }
                for (arm, start) in [("consequence", t), ("alternative", f)] {
                    let mut c = Some(start);
                    if let Some(a) = e.child_by_field_name(arm) {
                        c = self.expr(a, c);
                        if self.is_noreturn_call(a) {
                            let s = self.stmt_node(c);
                            self.edge(s, EXIT);
                            c = None;
                        } else {
                            c = Some(self.stmt_node(c));
                        }
                    }
                    if let Some(x) = c {
                        self.edge(x, join);
                    }
                }
                Some(join)
            }
            "sizeof_expression" | "alignof_expression" | "offsetof_expression" => cur,
            _ => {
                let mut cur = cur;
                for ch in Self::named_children(e) {
                    cur = self.expr(ch, cur);
                }
                cur
            }
        }
    }

    /// Branch on `c` from `cur` to `t` (true) / `f` (false).
    fn cond(&mut self, c: Node, cur: Option<u32>, t: u32, f: u32) {
        let c = strip_parens(c);
        match c.kind() {
            "binary_expression" if op(c) == Some("&&") => {
                let mid = self.node(false);
                if let Some(l) = c.child_by_field_name("left") {
                    self.cond(l, cur, mid, f);
                }
                if let Some(r) = c.child_by_field_name("right") {
                    self.cond(r, Some(mid), t, f);
                }
            }
            "binary_expression" if op(c) == Some("||") => {
                let mid = self.node(false);
                if let Some(l) = c.child_by_field_name("left") {
                    self.cond(l, cur, t, mid);
                }
                if let Some(r) = c.child_by_field_name("right") {
                    self.cond(r, Some(mid), t, f);
                }
            }
            "unary_expression" if op(c) == Some("!") => match c.child_by_field_name("argument") {
                Some(a) => self.cond(a, cur, f, t),
                None => self.branch(cur, t, f),
            },
            "comma_expression" => {
                // (a, b): a for effect, b decides
                let left = c.child_by_field_name("left");
                let mut cc = cur;
                if let Some(l) = left {
                    cc = self.expr_stmt(l, cc);
                }
                match c.child_by_field_name("right") {
                    Some(r) => self.cond(r, cc, t, f),
                    None => self.branch(cc, t, f),
                }
            }
            _ => match literal_truth(c, self.src) {
                Some(true) => {
                    let cc = self.expr(c, cur);
                    if let Some(x) = cc {
                        self.edge(x, t);
                    }
                }
                Some(false) => {
                    let cc = self.expr(c, cur);
                    if let Some(x) = cc {
                        self.edge(x, f);
                    }
                }
                None => {
                    let cc = self.expr(c, cur);
                    let b = self.stmt_node(cc);
                    self.edge(b, t);
                    self.edge(b, f);
                }
            },
        }
    }

    fn branch(&mut self, cur: Option<u32>, t: u32, f: u32) {
        let b = self.stmt_node(cur);
        self.edge(b, t);
        self.edge(b, f);
    }
}

fn op<'a>(n: Node<'a>) -> Option<&'static str> {
    n.child_by_field_name("operator").map(|o| o.kind())
}

fn strip_parens(mut n: Node) -> Node {
    while n.kind() == "parenthesized_expression" {
        match n.named_child(0) {
            Some(c) => n = c,
            None => break,
        }
    }
    n
}

fn literal_truth(n: Node, src: &str) -> Option<bool> {
    let n = strip_parens(n);
    match n.kind() {
        "true" => Some(true),
        "false" => Some(false),
        "number_literal" => crate::csyntax::int_value(text(n, src)).map(|v| v != 0),
        "cast_expression" => n
            .child_by_field_name("value")
            .and_then(|v| literal_truth(v, src)),
        _ => None,
    }
}

/// CFG of a function_definition node.
pub fn build(def: Node, src: &str) -> Graph {
    let mut b = B::new(src);
    let end = match def.child_by_field_name("body") {
        Some(body) => b.stmt(body, Some(ENTRY)),
        None => Some(ENTRY),
    };
    if let Some(e) = end {
        b.edge(e, EXIT);
    }
    finish(b.full, b.succ)
}

/// Remove empty pass-through nodes, drop unreachable nodes, merge chains, renumber.
/// Linear in nodes + edges.
fn finish(full: Vec<bool>, succ: Vec<BTreeSet<u32>>) -> Graph {
    let n = full.len();
    let mut full = full;
    full[ENTRY as usize] = true;
    full[EXIT as usize] = true;
    // 1) empty nodes on a cycle of empty nodes (`for(;;){}`) are kept: that loop is real
    for c in empty_cycles(&full, &succ) {
        full[c] = true;
    }
    // 2) every edge into an empty node goes to the non-empty nodes it reaches (memoized; acyclic now)
    let mut resolved: Vec<Option<BTreeSet<u32>>> = vec![None; n];
    fn resolve(
        e: usize,
        full: &[bool],
        succ: &[BTreeSet<u32>],
        memo: &mut [Option<BTreeSet<u32>>],
    ) -> BTreeSet<u32> {
        if let Some(r) = &memo[e] {
            return r.clone();
        }
        // iterative post-order over empty nodes
        let mut stack = vec![(e, false)];
        while let Some((x, done)) = stack.pop() {
            if memo[x].is_some() {
                continue;
            }
            if done {
                let mut out = BTreeSet::new();
                for &s in &succ[x] {
                    if full[s as usize] {
                        out.insert(s);
                    } else if let Some(r) = &memo[s as usize] {
                        out.extend(r.iter().copied());
                    }
                }
                memo[x] = Some(out);
            } else {
                stack.push((x, true));
                for &s in &succ[x] {
                    if !full[s as usize] && memo[s as usize].is_none() {
                        stack.push((s as usize, false));
                    }
                }
            }
        }
        memo[e].clone().unwrap()
    }
    let mut out: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); n];
    for a in 0..n {
        if !full[a] {
            continue;
        }
        for &s in &succ[a] {
            if full[s as usize] {
                out[a].insert(s);
            } else {
                out[a].extend(resolve(s as usize, &full, &succ, &mut resolved));
            }
        }
    }
    let mut succ = out;
    // 3) reachability from ENTRY; EXIT always stays
    let mut alive = vec![false; n];
    let mut stack = vec![ENTRY];
    while let Some(x) = stack.pop() {
        if alive[x as usize] {
            continue;
        }
        alive[x as usize] = true;
        stack.extend(succ[x as usize].iter().copied());
    }
    alive[EXIT as usize] = true;
    for a in 0..n {
        if !alive[a] {
            succ[a].clear();
        }
    }
    // 4) merge chains: u -> v, u's only successor, v's only predecessor (never EXIT, never into ENTRY)
    let mut preds = vec![0u32; n];
    for a in 0..n {
        for &s in &succ[a] {
            preds[s as usize] += 1;
        }
    }
    for u in 0..n {
        while alive[u] && u != EXIT as usize && succ[u].len() == 1 {
            let v = *succ[u].iter().next().unwrap() as usize;
            if v == u || v == EXIT as usize || v == ENTRY as usize || preds[v] != 1 {
                break;
            }
            let mut moved = std::mem::take(&mut succ[v]);
            if moved.remove(&(v as u32)) {
                moved.insert(u as u32); // v's self-loop becomes u's
            }
            succ[u] = moved;
            alive[v] = false;
        }
    }
    // 5) renumber: ENTRY, then DFS preorder visiting successors in id order, EXIT last
    let mut order: Vec<usize> = Vec::new();
    let mut seen = vec![false; n];
    let mut st = vec![ENTRY as usize];
    while let Some(x) = st.pop() {
        if seen[x] || !alive[x] || x == EXIT as usize {
            continue;
        }
        seen[x] = true;
        order.push(x);
        for &s in succ[x].iter().rev() {
            st.push(s as usize);
        }
    }
    order.push(EXIT as usize);
    let mut id = vec![u32::MAX; n];
    for (i, &x) in order.iter().enumerate() {
        id[x] = i as u32;
    }
    let labels = order
        .iter()
        .map(|&x| match x as u32 {
            ENTRY => Label::Entry,
            EXIT => Label::Exit,
            _ => Label::Block,
        })
        .collect();
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for &x in &order {
        for &s in &succ[x] {
            if id[s as usize] != u32::MAX {
                edges.push((id[x], id[s as usize]));
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    Graph { labels, edges }
}

/// Empty nodes that lie on a cycle made only of empty nodes (Tarjan SCC, iterative).
fn empty_cycles(full: &[bool], succ: &[BTreeSet<u32>]) -> Vec<usize> {
    let n = full.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on = vec![false; n];
    let mut st: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    let mut next = 0usize;
    for root in 0..n {
        if full[root] || index[root] != usize::MAX {
            continue;
        }
        let mut call: Vec<(usize, Vec<usize>, usize)> = Vec::new();
        let kids = |v: usize| -> Vec<usize> {
            succ[v]
                .iter()
                .map(|&s| s as usize)
                .filter(|&s| !full[s])
                .collect()
        };
        index[root] = next;
        low[root] = next;
        next += 1;
        st.push(root);
        on[root] = true;
        call.push((root, kids(root), 0));
        while let Some((v, ks, i)) = call.last_mut() {
            let v = *v;
            if *i < ks.len() {
                let w = ks[*i];
                *i += 1;
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    st.push(w);
                    on[w] = true;
                    let kw = kids(w);
                    call.push((w, kw, 0));
                } else if on[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                call.pop();
                if let Some((p, _, _)) = call.last() {
                    let p = *p;
                    low[p] = low[p].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let w = st.pop().unwrap();
                        on[w] = false;
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    let cyclic = comp.len() > 1 || succ[v].contains(&(v as u32));
                    if cyclic {
                        out.extend(comp);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csyntax::{output_function, parse};

    fn g(code: &str) -> Graph {
        let t = parse(code).unwrap();
        let def = output_function(&t, code, "f").expect("function f");
        build(def, code)
    }

    fn shape(gr: &Graph) -> (usize, usize) {
        (gr.nodes(), gr.edges.len())
    }

    #[test]
    fn straight_line_is_entry_exit() {
        let x = g("int f(int a){ int b = a + 1; b *= 2; return b; }");
        assert_eq!(x.labels, [Label::Entry, Label::Exit]);
        assert_eq!(x.edges, [(0, 1)]);
    }

    #[test]
    fn if_else_diamond() {
        // entry(cond) -> then, else; both -> return block -> exit
        let x = g("int f(int a){ int r; if (a) r = 1; else r = 2; return r; }");
        assert_eq!(shape(&x), (5, 5));
    }

    #[test]
    fn short_circuit_equals_nested_if() {
        let a = g("int f(int a, int b){ if (a && b) g(); return 0; }");
        let b = g("int f(int a, int b){ if (a) { if (b) g(); } return 0; }");
        assert_eq!(a, b);
        let c = g("int f(int a, int b){ if (a || b) g(); return 0; }");
        assert_eq!(shape(&c), shape(&a));
    }

    #[test]
    fn ternary_equals_if_else() {
        let a = g("int f(int c){ int x; x = c ? 1 : 2; return x; }");
        let b = g("int f(int c){ int x; if (c) x = 1; else x = 2; return x; }");
        assert_eq!(a, b);
    }

    #[test]
    fn literal_conditions_fold() {
        let a = g("int f(int a){ do { a++; } while (0); return a; }");
        assert_eq!(a.labels, [Label::Entry, Label::Exit]);
        let b = g("int f(int a){ while (1) { if (a) break; a++; } return a; }");
        let c = g("int f(int a){ for (;;) { if (a) break; a++; } return a; }");
        assert_eq!(b, c);
    }

    #[test]
    fn returns_and_noreturn_reach_exit() {
        let x = g("int f(int a){ if (a) exit(1); return 0; }");
        // entry -> {exit(1)} -> EXIT, entry -> {return} -> EXIT
        assert_eq!(shape(&x), (4, 4));
        assert!(x.edges.contains(&(0, 1)) || x.edges.contains(&(0, 2)));
    }

    #[test]
    fn goto_and_labels() {
        let a = g("int f(int a){ if (a) goto L; a = 2; L: return a; }");
        let b = g("int f(int a){ if (!a) { a = 2; } return a; }");
        assert_eq!(a, b);
    }

    #[test]
    fn switch_with_and_without_default() {
        let a = g("int f(int a){ switch (a) { case 1: a = 5; break; case 2: case 3: a = 6; break; } return a; }");
        // switch node -> case1, case2/3 (one target after merging the empty case 2), after
        let sw_out = a.edges.iter().filter(|(s, _)| *s == 0).count();
        assert_eq!(sw_out, 3);
        let b = g("int f(int a){ switch (a) { case 1: a = 5; break; default: a = 6; } return a; }");
        assert_eq!(b.edges.iter().filter(|(s, _)| *s == 0).count(), 2);
    }

    #[test]
    fn loops_have_back_edges() {
        let w = g("int f(int n){ int s = 0; while (n) { s += n; n--; } return s; }");
        let has_back = w
            .edges
            .iter()
            .any(|(a, b)| b <= a && w.labels[*b as usize] != Label::Exit);
        assert!(has_back);
        let d = g("int f(int n){ int s = 0; do { s += n; n--; } while (n); return s; }");
        assert!(d.edges.iter().any(|(a, b)| b <= a));
    }

    #[test]
    fn unreachable_code_is_dropped() {
        let a = g("int f(int a){ return a; a = 3; }");
        assert_eq!(a.labels, [Label::Entry, Label::Exit]);
    }

    #[test]
    fn deterministic() {
        let code = "int f(int a){ for (int i = 0; i < a; i++) { if (i & 1) continue; g(i); } switch (a) { case 0: return 1; default: break; } return a ? 2 : 3; }";
        assert_eq!(g(code), g(code));
    }
}

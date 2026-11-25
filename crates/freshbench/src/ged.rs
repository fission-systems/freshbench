//! Graph edit distance between two CFGs, deterministic.
//!
//! Cost model: inserting or deleting a node costs 1, inserting or deleting an
//! edge costs 1, substituting a node costs 0 when the labels (ENTRY, EXIT,
//! block) agree and 1 otherwise. Edges are unlabeled and directed.
//!
//! Method:
//! 1. Lower bound `|n1-n2| + |e1-e2|` (every unit of size difference needs one
//!    edit).
//! 2. Upper bound from a node assignment: Hungarian assignment on a cost
//!    matrix of label and degree differences (Riesen-Bunke bipartite GED) for
//!    graphs up to [`HUNGARIAN_MAX`] nodes combined, a degree-greedy assignment
//!    above; the edit cost the assignment induces is computed exactly.
//! 3. Exact search: depth-first branch and bound over mappings of graph 1's
//!    nodes in DFS order, with the bound from (1) applied to what is left,
//!    starting from the upper bound of (2). The search is limited by a count of
//!    expanded states ([`DEFAULT_BUDGET`]), never by time, so the same pair
//!    always gives the same answer on any machine. If it finishes, the value is
//!    exact; otherwise it is the best mapping found, an upper bound.
//!
//! A value of 0 is always exact (an isomorphism was found).

use serde::Serialize;

use crate::cfg::{Graph, Label};

pub const DEFAULT_BUDGET: u64 = 200_000;
pub const HUNGARIAN_MAX: usize = 400;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ged {
    pub value: u64,
    pub exact: bool,
    pub lower_bound: u64,
    pub method: &'static str,
    pub expanded: u64,
}

struct Adj {
    n: usize,
    labels: Vec<Label>,
    out: Vec<Vec<u32>>,
    inn: Vec<Vec<u32>>,
    edges: Vec<(u32, u32)>,
    has: std::collections::HashSet<(u32, u32)>,
}

impl Adj {
    fn new(g: &Graph) -> Adj {
        let n = g.nodes();
        let mut out = vec![vec![]; n];
        let mut inn = vec![vec![]; n];
        for &(a, b) in &g.edges {
            out[a as usize].push(b);
            inn[b as usize].push(a);
        }
        Adj {
            n,
            labels: g.labels.clone(),
            out,
            inn,
            edges: g.edges.clone(),
            has: g.edges.iter().copied().collect(),
        }
    }
    fn edge(&self, a: u32, b: u32) -> bool {
        self.has.contains(&(a, b))
    }
}

const DEL: u32 = u32::MAX;

/// Exact edit cost of a complete mapping (`map[u]` = node of g2, or DEL).
fn mapping_cost(g1: &Adj, g2: &Adj, map: &[u32]) -> u64 {
    let mut cost = 0u64;
    let mut used = vec![false; g2.n];
    for (u, &v) in map.iter().enumerate() {
        if v == DEL {
            cost += 1;
        } else {
            used[v as usize] = true;
            if g1.labels[u] != g2.labels[v as usize] {
                cost += 1;
            }
        }
    }
    cost += used.iter().filter(|x| !**x).count() as u64;
    let mut kept = 0u64;
    for &(a, b) in &g1.edges {
        let (x, y) = (map[a as usize], map[b as usize]);
        if x != DEL && y != DEL && g2.edge(x, y) {
            kept += 1;
        }
    }
    cost + (g1.edges.len() as u64 - kept) + (g2.edges.len() as u64 - kept)
}

pub fn lower_bound(g1: &Graph, g2: &Graph) -> u64 {
    (g1.nodes().abs_diff(g2.nodes()) + g1.edges.len().abs_diff(g2.edges.len())) as u64
}

/// Node assignment from a bipartite cost matrix (Hungarian) or greedy above the size limit.
fn assignment(g1: &Adj, g2: &Adj) -> Vec<u32> {
    let (n1, n2) = (g1.n, g2.n);
    let sub = |u: usize, v: usize| -> i64 {
        let l = i64::from(g1.labels[u] != g2.labels[v]);
        let d = (g1.out[u].len() as i64 - g2.out[v].len() as i64).abs()
            + (g1.inn[u].len() as i64 - g2.inn[v].len() as i64).abs();
        2 * l + d // doubled to stay integral: edge costs shared by two endpoints
    };
    let del = |u: usize| -> i64 { 2 + (g1.out[u].len() + g1.inn[u].len()) as i64 };
    let ins = |v: usize| -> i64 { 2 + (g2.out[v].len() + g2.inn[v].len()) as i64 };
    if n1 + n2 <= HUNGARIAN_MAX {
        let size = n1 + n2;
        const INF: i64 = i64::MAX / 4;
        let mut c = vec![vec![0i64; size]; size];
        for i in 0..size {
            for j in 0..size {
                c[i][j] = match (i < n1, j < n2) {
                    (true, true) => sub(i, j),
                    (true, false) => {
                        if j - n2 == i {
                            del(i)
                        } else {
                            INF
                        }
                    }
                    (false, true) => {
                        if i - n1 == j {
                            ins(j)
                        } else {
                            INF
                        }
                    }
                    (false, false) => 0,
                };
            }
        }
        let a = hungarian(&c);
        (0..n1)
            .map(|i| if a[i] < n2 { a[i] as u32 } else { DEL })
            .collect()
    } else {
        // greedy: in node order, the unused g2 node of least substitution cost (ties: lowest id)
        let mut used = vec![false; n2];
        let mut map = vec![DEL; n1];
        for u in 0..n1 {
            let mut best: Option<(i64, usize)> = None;
            for v in 0..n2 {
                if used[v] {
                    continue;
                }
                let s = sub(u, v);
                if best.is_none_or(|(b, _)| s < b) {
                    best = Some((s, v));
                }
            }
            match best {
                Some((s, v)) if s < del(u) + ins(v) => {
                    used[v] = true;
                    map[u] = v as u32;
                }
                _ => {}
            }
        }
        map
    }
}

/// Minimum-cost perfect assignment of a square matrix (Kuhn-Munkres, O(n^3), deterministic).
fn hungarian(c: &[Vec<i64>]) -> Vec<usize> {
    let n = c.len();
    let inf = i64::MAX / 4;
    let (mut u, mut v) = (vec![0i64; n + 1], vec![0i64; n + 1]);
    let (mut p, mut way) = (vec![0usize; n + 1], vec![0usize; n + 1]);
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![inf; n + 1];
        let mut used = vec![false; n + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = 0usize;
            for j in 1..=n {
                if !used[j] {
                    let cur = c[i0 - 1][j - 1].saturating_sub(u[i0]).saturating_sub(v[j]);
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=n {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut ans = vec![0usize; n];
    for j in 1..=n {
        if p[j] > 0 {
            ans[p[j] - 1] = j - 1;
        }
    }
    ans
}

/// Deterministic hill climbing on a complete mapping: for each g1 node in id
/// order, the best of swapping images with another g1 node, moving to an unused
/// g2 node, or deletion; repeated until no move improves or `max_passes`.
/// Cost deltas are computed from the edges incident to the moved nodes only.
fn local_search(g1: &Adj, g2: &Adj, map: &mut [u32], max_passes: usize, work_limit: u64) -> u64 {
    let n1 = g1.n;
    let mut used = vec![DEL; g2.n]; // g2 node -> g1 node using it
    for (u, &v) in map.iter().enumerate() {
        if v != DEL {
            used[v as usize] = u as u32;
        }
    }
    let node_term = |u: usize, v: u32| -> i64 {
        if v == DEL {
            1
        } else {
            i64::from(g1.labels[u] != g2.labels[v as usize])
        }
    };
    // kept g1 edges incident to the nodes in `s` (each edge once)
    let kept = |map: &[u32], s: &[usize]| -> i64 {
        let mut es: Vec<(u32, u32)> = Vec::with_capacity(16);
        for &u in s {
            es.extend(g1.out[u].iter().map(|&w| (u as u32, w)));
            es.extend(g1.inn[u].iter().map(|&w| (w, u as u32)));
        }
        es.sort_unstable();
        es.dedup();
        es.iter()
            .filter(|&&(a, b)| {
                let (x, y) = (map[a as usize], map[b as usize]);
                x != DEL && y != DEL && g2.edge(x, y)
            })
            .count() as i64
    };
    #[cfg(debug_assertions)]
    let mut current = mapping_cost(g1, g2, map) as i64;
    let mut work = 0u64;
    for _ in 0..max_passes {
        let mut improved = false;
        for u in 0..n1 {
            let vu = map[u];
            let mut best: (i64, u8, u32) = (0, 0, 0); // (delta, kind 1=swap 2=move, target)
                                                      // swaps
            for u2 in 0..n1 {
                if u2 == u || map[u2] == vu {
                    continue;
                }
                let s = [u, u2];
                let before = node_term(u, map[u]) + node_term(u2, map[u2]) - 2 * kept(map, &s);
                map.swap(u, u2);
                let after = node_term(u, map[u]) + node_term(u2, map[u2]) - 2 * kept(map, &s);
                map.swap(u, u2);
                work += 1;
                if after - before < best.0 {
                    best = (after - before, 1, u2 as u32);
                }
            }
            // moves to unused g2 nodes or deletion
            for v in (0..g2.n as u32)
                .filter(|&v| used[v as usize] == DEL)
                .chain([DEL])
            {
                if v == vu {
                    continue;
                }
                let s = [u];
                let unused_delta = i64::from(vu != DEL) - i64::from(v != DEL);
                let before = node_term(u, vu) - 2 * kept(map, &s);
                map[u] = v;
                let after = node_term(u, v) + unused_delta - 2 * kept(map, &s);
                map[u] = vu;
                work += 1;
                if after - before < best.0 {
                    best = (after - before, 2, v);
                }
            }
            #[cfg(debug_assertions)]
            if best.1 != 0 {
                current += best.0;
            }
            match best.1 {
                1 => {
                    let u2 = best.2 as usize;
                    map.swap(u, u2);
                    for &x in &[u, u2] {
                        if map[x] != DEL {
                            used[map[x] as usize] = x as u32;
                        }
                    }
                    improved = true;
                }
                2 => {
                    if vu != DEL {
                        used[vu as usize] = DEL;
                    }
                    map[u] = best.2;
                    if best.2 != DEL {
                        used[best.2 as usize] = u as u32;
                    }
                    improved = true;
                }
                _ => {}
            }
            #[cfg(debug_assertions)]
            debug_assert_eq!(
                current,
                mapping_cost(g1, g2, map) as i64,
                "incremental delta disagrees with the full cost"
            );
            if work > work_limit {
                return mapping_cost(g1, g2, map);
            }
        }
        if !improved {
            break;
        }
    }
    mapping_cost(g1, g2, map)
}

struct Search<'a> {
    g1: &'a Adj,
    g2: &'a Adj,
    order: Vec<u32>,
    pos: Vec<usize>, // position of each g1 node in order
    map: Vec<u32>,
    used: Vec<bool>,
    inv: Vec<u32>,
    n2_used: usize,
    best: u64,
    best_map: Vec<u32>,
    /// the starting mapping: its image is tried first at every level
    hint: Vec<u32>,
    expanded: u64,
    budget: u64,
    exhausted: bool,
    e1_total: usize,
    e2_total: usize,
}

impl Search<'_> {
    /// Cost of placing order[k] = u as v (or DEL), counting edges to earlier-placed g1 nodes.
    fn step_cost(&self, k: usize, u: u32, v: u32) -> (u64, usize, usize) {
        // returns (cost, g1 edges now decided, g2 edges now decided)
        let mut cost = 0u64;
        let (mut d1, mut d2) = (0usize, 0usize);
        // deletion, or a substitution between different labels
        if v == DEL || self.g1.labels[u as usize] != self.g2.labels[v as usize] {
            cost += 1;
        }
        // g1 edges between u and earlier nodes (and the self-loop)
        let handle1 = |a: u32, b: u32, cost: &mut u64, d1: &mut usize| {
            *d1 += 1;
            let (x, y) = (
                if a == u { v } else { self.map[a as usize] },
                if b == u { v } else { self.map[b as usize] },
            );
            if x == DEL || y == DEL || !self.g2.edge(x, y) {
                *cost += 1;
            }
        };
        for &w in &self.g1.out[u as usize] {
            if w == u || self.pos[w as usize] < k {
                handle1(u, w, &mut cost, &mut d1);
            }
        }
        for &w in &self.g1.inn[u as usize] {
            if w != u && self.pos[w as usize] < k {
                handle1(w, u, &mut cost, &mut d1);
            }
        }
        // g2 edges between v and images of earlier nodes, with no preimage edge
        if v != DEL {
            let inv = |x: u32| -> Option<u32> {
                if x == v {
                    Some(u)
                } else if self.used[x as usize] {
                    Some(self.inv[x as usize]) // image of an earlier g1 node
                } else {
                    None
                }
            };
            for &x in &self.g2.out[v as usize] {
                if let Some(px) = inv(x) {
                    d2 += 1;
                    if !self.g1.edge(u, px) {
                        cost += 1;
                    }
                }
            }
            for &x in &self.g2.inn[v as usize] {
                if x == v {
                    continue;
                }
                if let Some(px) = inv(x) {
                    d2 += 1;
                    if !self.g1.edge(px, u) {
                        cost += 1;
                    }
                }
            }
        }
        (cost, d1, d2)
    }

    fn dfs(&mut self, k: usize, cost: u64, e1_done: usize, e2_done: usize) {
        if self.expanded >= self.budget {
            self.exhausted = true;
            return;
        }
        let n1_left = self.order.len() - k;
        let n2_left = self.g2.n - self.n2_used;
        if k == self.order.len() {
            // remaining g2 nodes inserted with all their undecided edges
            let total = cost + n2_left as u64 + (self.e2_total - e2_done) as u64;
            if total < self.best {
                self.best = total;
                self.best_map = self.map.clone();
            }
            return;
        }
        let lb = cost
            + n1_left.abs_diff(n2_left) as u64
            + (self.e1_total - e1_done).abs_diff(self.e2_total - e2_done) as u64;
        if lb >= self.best {
            return;
        }
        let u = self.order[k];
        // candidates: unused g2 nodes by (substitution cost, degree difference, id), then deletion
        let mut cands: Vec<(bool, u64, usize, u32)> = Vec::new();
        for v in 0..self.g2.n as u32 {
            if self.used[v as usize] {
                continue;
            }
            let (c, _, _) = self.step_cost(k, u, v);
            let dd = self.g1.out[u as usize]
                .len()
                .abs_diff(self.g2.out[v as usize].len())
                + self.g1.inn[u as usize]
                    .len()
                    .abs_diff(self.g2.inn[v as usize].len());
            cands.push((v != self.hint[u as usize], c, dd, v));
        }
        // work is counted per candidate evaluated, so large graphs stop early, deterministically
        self.expanded += 1 + cands.len() as u64;
        cands.sort_unstable();
        for (_, _, _, v) in cands {
            let (c, d1, d2) = self.step_cost(k, u, v);
            if cost + c >= self.best {
                continue;
            }
            self.map[u as usize] = v;
            self.used[v as usize] = true;
            self.inv[v as usize] = u;
            self.n2_used += 1;
            self.dfs(k + 1, cost + c, e1_done + d1, e2_done + d2);
            self.n2_used -= 1;
            self.used[v as usize] = false;
            self.map[u as usize] = DEL;
            if self.best == 0 || self.exhausted {
                return;
            }
        }
        let (c, d1, d2) = self.step_cost(k, u, DEL);
        if cost + c < self.best {
            self.map[u as usize] = DEL;
            self.dfs(k + 1, cost + c, e1_done + d1, e2_done + d2);
        }
    }
}

/// DFS preorder from node 0, then any unvisited nodes in id order.
fn dfs_order(g: &Adj) -> Vec<u32> {
    let mut seen = vec![false; g.n];
    let mut order = Vec::with_capacity(g.n);
    for root in 0..g.n {
        let mut st = vec![root as u32];
        while let Some(x) = st.pop() {
            if seen[x as usize] {
                continue;
            }
            seen[x as usize] = true;
            order.push(x);
            for &s in g.out[x as usize].iter().rev() {
                st.push(s);
            }
        }
    }
    order
}

/// Steps the isomorphism search may take; independent of the GED budget.
pub const ISO_CAP: u64 = 5_000_000;

/// Are the two graphs isomorphic as directed graphs with entry/exit/block
/// labels? `Some(answer)`, or `None` when the search took more than `cap`
/// steps (then nothing is known).
///
/// GED 0 means isomorphic, and a budget-limited GED search can stop above 0 on
/// two large isomorphic graphs. This check does not share that budget: colour
/// refinement (Weisfeiler-Lehman over labels, in- and out-neighbours) splits
/// the nodes into classes, which decides most pairs outright, and a
/// backtracking search maps the rest class by class.
pub fn isomorphic(a: &Graph, b: &Graph, cap: u64) -> Option<bool> {
    use std::collections::{BTreeMap, HashSet};
    let n = a.nodes();
    if n != b.nodes() || a.edges.len() != b.edges.len() {
        return Some(false);
    }
    let adj = |g: &Graph| {
        let (mut out, mut inn) = (vec![Vec::new(); n], vec![Vec::new(); n]);
        for &(x, y) in &g.edges {
            out[x as usize].push(y as usize);
            inn[y as usize].push(x as usize);
        }
        (out, inn)
    };
    let ((oa, ia), (ob, ib)) = (adj(a), adj(b));
    // colours shared by both graphs: ids of sorted signatures
    let label = |l: &Label| match l {
        Label::Entry => 0u64,
        Label::Exit => 1,
        Label::Block => 2,
    };
    let mut ca: Vec<u64> = a.labels.iter().map(label).collect();
    let mut cb: Vec<u64> = b.labels.iter().map(label).collect();
    let mut classes = 0;
    loop {
        let sig = |c: &[u64], out: &[Vec<usize>], inn: &[Vec<usize>], v: usize| {
            let mut o: Vec<u64> = out[v].iter().map(|&w| c[w]).collect();
            let mut i: Vec<u64> = inn[v].iter().map(|&w| c[w]).collect();
            o.sort_unstable();
            i.sort_unstable();
            (c[v], o, i)
        };
        let sa: Vec<_> = (0..n).map(|v| sig(&ca, &oa, &ia, v)).collect();
        let sb: Vec<_> = (0..n).map(|v| sig(&cb, &ob, &ib, v)).collect();
        let mut ids = BTreeMap::new();
        for s in sa.iter().chain(sb.iter()) {
            let next = ids.len() as u64;
            ids.entry(s.clone()).or_insert(next);
        }
        ca = sa.iter().map(|s| ids[s]).collect();
        cb = sb.iter().map(|s| ids[s]).collect();
        let (mut ha, mut hb) = (ca.clone(), cb.clone());
        ha.sort_unstable();
        hb.sort_unstable();
        if ha != hb {
            return Some(false);
        }
        if ids.len() == classes {
            break;
        }
        classes = ids.len();
    }
    // map a's nodes in order of rarest colour first
    let mut size: BTreeMap<u64, usize> = BTreeMap::new();
    for &c in &ca {
        *size.entry(c).or_default() += 1;
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&v| (size[&ca[v]], v));
    let eb: HashSet<(usize, usize)> = b
        .edges
        .iter()
        .map(|&(x, y)| (x as usize, y as usize))
        .collect();
    let mut by_colour: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (v, &c) in cb.iter().enumerate() {
        by_colour.entry(c).or_default().push(v);
    }
    const NONE: usize = usize::MAX;
    let (mut map, mut used) = (vec![NONE; n], vec![false; n]);
    let mut steps = 0u64;
    // iterative backtracking: choice[i] = index into the candidate list tried at depth i
    let mut choice = vec![0usize; n];
    let mut depth = 0usize;
    loop {
        if depth == n {
            return Some(true);
        }
        let u = order[depth];
        let cands = &by_colour[&ca[u]];
        let mut placed = false;
        while choice[depth] < cands.len() {
            let v = cands[choice[depth]];
            choice[depth] += 1;
            steps += 1;
            if steps > cap {
                return None;
            }
            if used[v] {
                continue;
            }
            // every edge of a between u and an already mapped node (or u itself) is in b;
            // with equal edge counts and a bijection that is edge equality
            let ok = oa[u].iter().all(|&w| {
                let t = if w == u { v } else { map[w] };
                t == NONE || eb.contains(&(v, t))
            }) && ia[u].iter().all(|&w| {
                let t = if w == u { v } else { map[w] };
                t == NONE || eb.contains(&(t, v))
            });
            if ok {
                map[u] = v;
                used[v] = true;
                placed = true;
                break;
            }
        }
        if placed {
            depth += 1;
            if depth < n {
                choice[depth] = 0;
            }
        } else {
            if depth == 0 {
                return Some(false);
            }
            depth -= 1;
            let pu = order[depth];
            used[map[pu]] = false;
            map[pu] = NONE;
        }
    }
}

pub fn distance(a: &Graph, b: &Graph) -> Ged {
    distance_with_budget(a, b, DEFAULT_BUDGET)
}

pub fn distance_with_budget(a: &Graph, b: &Graph, budget: u64) -> Ged {
    // GED 0 is decided by its own check, whatever the budget
    let iso = isomorphic(a, b, ISO_CAP);
    if iso == Some(true) {
        return Ged {
            value: 0,
            exact: true,
            lower_bound: 0,
            method: "isomorphism",
            expanded: 0,
        };
    }
    let (g1, g2) = (Adj::new(a), Adj::new(b));
    // not isomorphic: at least one edit
    let lb = lower_bound(a, b).max(u64::from(iso == Some(false)));
    let mut init = assignment(&g1, &g2);
    let ub = local_search(&g1, &g2, &mut init, 50, budget.saturating_mul(4));
    let method = match (g1.n + g2.n <= HUNGARIAN_MAX, iso.is_none()) {
        (true, false) => "bipartite",
        (false, false) => "greedy",
        // the isomorphism check ran out of steps: GED 0 is not ruled out
        (true, true) => "bipartite, isomorphism unknown",
        (false, true) => "greedy, isomorphism unknown",
    };
    if ub == lb {
        return Ged {
            value: ub,
            exact: true,
            lower_bound: lb,
            method,
            expanded: 0,
        };
    }
    let order = dfs_order(&g1);
    let mut pos = vec![0usize; g1.n];
    for (i, &u) in order.iter().enumerate() {
        pos[u as usize] = i;
    }
    let mut s = Search {
        g1: &g1,
        g2: &g2,
        order,
        pos,
        map: vec![DEL; g1.n],
        used: vec![false; g2.n],
        inv: vec![DEL; g2.n],
        n2_used: 0,
        best: ub,
        hint: init.clone(),
        best_map: init,
        expanded: 0,
        budget,
        exhausted: false,
        e1_total: g1.edges.len(),
        e2_total: g2.edges.len(),
    };
    s.dfs(0, 0, 0, 0);
    debug_assert_eq!(mapping_cost(&g1, &g2, &s.best_map), s.best);
    let exact = !s.exhausted || s.best == lb;
    Ged {
        value: s.best,
        exact,
        lower_bound: if exact { s.best } else { lb },
        method: if exact { "exact" } else { method },
        expanded: s.expanded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(n: usize, edges: &[(u32, u32)]) -> Graph {
        let mut labels = vec![Label::Block; n];
        labels[0] = Label::Entry;
        labels[n - 1] = Label::Exit;
        let mut e = edges.to_vec();
        e.sort_unstable();
        Graph { labels, edges: e }
    }

    #[test]
    fn identical_is_zero() {
        let g = graph(5, &[(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)]);
        let d = distance(&g, &g);
        assert_eq!((d.value, d.exact, d.lower_bound), (0, true, 0));
    }

    #[test]
    fn isomorphic_relabelled_is_zero() {
        let a = graph(5, &[(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)]);
        let b = graph(5, &[(0, 2), (0, 1), (2, 3), (1, 3), (3, 4)]);
        assert_eq!(distance(&a, &b).value, 0);
        // a loop placed differently in the numbering
        let c = graph(4, &[(0, 1), (1, 2), (2, 1), (1, 3)]);
        let d = graph(4, &[(0, 2), (2, 1), (1, 2), (2, 3)]);
        assert_eq!(distance(&c, &d).value, 0);
    }

    #[test]
    fn one_extra_node_and_edge() {
        // diamond vs diamond with the join split into two blocks
        let a = graph(5, &[(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)]);
        let b = graph(6, &[(0, 1), (0, 2), (1, 3), (2, 4), (3, 5), (4, 5)]);
        // insert one node, two edges; delete one edge (worked by hand)
        let d = distance(&a, &b);
        assert!(d.exact);
        assert_eq!(d.value, 4);
        assert_eq!(lower_bound(&a, &b), 2);
    }

    #[test]
    fn symmetric_and_label_cost() {
        let a = graph(3, &[(0, 1), (1, 2)]);
        let b = graph(2, &[(0, 1)]);
        let (x, y) = (distance(&a, &b), distance(&b, &a));
        assert_eq!(x.value, y.value);
        // keep ENTRY, substitute the block for EXIT (1), delete EXIT (1) and its edge (1)
        assert_eq!(x.value, 3);
        assert!(x.exact);
    }

    #[test]
    fn upper_bound_never_below_exact_and_budget_is_deterministic() {
        let a = graph(
            9,
            &[
                (0, 1),
                (1, 2),
                (2, 3),
                (3, 1),
                (1, 4),
                (4, 5),
                (5, 6),
                (6, 4),
                (4, 7),
                (7, 8),
                (0, 8),
            ],
        );
        let b = graph(
            8,
            &[
                (0, 1),
                (1, 2),
                (2, 1),
                (1, 3),
                (3, 4),
                (4, 5),
                (5, 3),
                (3, 6),
                (6, 7),
            ],
        );
        let exact = distance(&a, &b);
        assert!(exact.exact);
        let tiny = distance_with_budget(&a, &b, 3);
        assert!(tiny.value >= exact.value);
        assert_eq!(tiny, distance_with_budget(&a, &b, 3));
    }

    #[test]
    fn local_search_never_worsens_and_matches_full_cost() {
        let a = graph(
            9,
            &[
                (0, 1),
                (1, 2),
                (2, 3),
                (3, 1),
                (1, 4),
                (4, 5),
                (5, 6),
                (6, 4),
                (4, 7),
                (7, 8),
                (0, 8),
            ],
        );
        let b = graph(
            8,
            &[
                (0, 1),
                (1, 2),
                (2, 1),
                (1, 3),
                (3, 4),
                (4, 5),
                (5, 3),
                (3, 6),
                (6, 7),
            ],
        );
        let (g1, g2) = (Adj::new(&a), Adj::new(&b));
        // a deliberately bad start: reversed images
        let mut map: Vec<u32> = (0..9)
            .map(|i| if i < 8 { 7 - i as u32 } else { DEL })
            .collect();
        let start = mapping_cost(&g1, &g2, &map);
        let got = local_search(&g1, &g2, &mut map, 50, u64::MAX);
        assert_eq!(got, mapping_cost(&g1, &g2, &map));
        assert!(got < start);
        assert!(got >= distance(&a, &b).value);
    }

    #[test]
    fn hungarian_small() {
        let c = vec![vec![4, 1, 3], vec![2, 0, 5], vec![3, 2, 2]];
        let a = hungarian(&c);
        let total: i64 = a.iter().enumerate().map(|(i, &j)| c[i][j]).sum();
        assert_eq!(total, 5);
    }

    /// DecBench #57: two large isomorphic CFGs must score 0 whatever the
    /// search budget is.
    #[test]
    fn large_isomorphic_graphs_are_perfect_without_budget() {
        let n = 400u32;
        // a CFG-like graph: a chain with forward branches and back edges
        let mut edges = Vec::new();
        for i in 0..n - 1 {
            edges.push((i, i + 1));
            if i % 3 == 0 && i + 7 < n {
                edges.push((i, i + 7));
            }
            if i % 11 == 5 && i >= 4 {
                edges.push((i, i - 4));
            }
        }
        let a = graph(n as usize, &edges);
        // the same graph with its nodes renumbered
        let perm: Vec<u32> = (0..n).map(|i| (i * 137 + 29) % n).collect();
        let mut labels = vec![Label::Block; n as usize];
        for (old, &new) in perm.iter().enumerate() {
            labels[new as usize] = a.labels[old];
        }
        let mut pe: Vec<(u32, u32)> = edges
            .iter()
            .map(|&(x, y)| (perm[x as usize], perm[y as usize]))
            .collect();
        pe.sort_unstable();
        let b = Graph { labels, edges: pe };
        assert_eq!(isomorphic(&a, &b, ISO_CAP), Some(true));
        let d = distance_with_budget(&a, &b, 1);
        assert_eq!((d.value, d.exact, d.method), (0, true, "isomorphism"));
        // one edge moved: not isomorphic, and never reported as 0
        let mut other = b.clone();
        let last = other.edges.len() - 1;
        other.edges[last].1 = (other.edges[last].1 + 1) % n;
        other.edges.sort_unstable();
        other.edges.dedup();
        if other.edges.len() == b.edges.len() {
            assert_eq!(isomorphic(&a, &other, ISO_CAP), Some(false));
            let d = distance_with_budget(&a, &other, 1);
            assert!(d.value > 0 && d.lower_bound >= 1);
        }
    }

    #[test]
    fn isomorphism_beyond_colour_refinement_and_the_cap() {
        let blocks = |n: usize, edges: &[(u32, u32)]| {
            let mut e = edges.to_vec();
            e.sort_unstable();
            Graph {
                labels: vec![Label::Block; n],
                edges: e,
            }
        };
        // one 6-cycle against two 3-cycles: every node looks the same to
        // colour refinement, only the search tells them apart
        let c6 = blocks(6, &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)]);
        let c33 = blocks(6, &[(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)]);
        assert_eq!(isomorphic(&c6, &c33, ISO_CAP), Some(false));
        assert_eq!(isomorphic(&c6, &c6, ISO_CAP), Some(true));
        // a rotated cycle is the same graph
        let rot = blocks(6, &[(1, 2), (2, 3), (3, 4), (4, 5), (5, 0), (0, 1)]);
        assert_eq!(isomorphic(&c6, &rot, ISO_CAP), Some(true));
        // out of steps: unknown, not "no"
        assert_eq!(isomorphic(&c6, &c33, 1), None);
        // a self loop has to map to a self loop
        let l1 = blocks(2, &[(0, 0), (0, 1)]);
        let l2 = blocks(2, &[(0, 1), (1, 1)]);
        assert_eq!(isomorphic(&l1, &l2, ISO_CAP), Some(false));
    }
}

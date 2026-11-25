//! Where a decompiler can improve, measured against the other decompilers.
//!
//! For one tool T and every row:
//! * `best_other`: the lowest GED any other tool scored on that row;
//! * reachable: another tool is perfect (GED 0) there and T is not - an
//!   existence proof that the source's shape is recoverable from the same
//!   stripped binary;
//! * gap: T's GED minus `best_other` when T scored and is farther;
//! * uncovered: T has no score where another tool scored;
//! * proven gap: T's GED *lower bound* minus `best_other` (an upper bound).
//!   GED values from a budget-limited search are upper bounds, so `gap` can
//!   overstate a difference; a proven gap cannot. A row with `gap > 0` and no
//!   proven gap is inconclusive, and a reachable row is proven only when T's
//!   lower bound is above 0.
//!
//! Rows are grouped by size band, outcome and tag. A row with several tags
//! counts under each (tag totals overlap). Examples are worst first, ties broken
//! by row id, so the report is deterministic. These are similarity gaps, not
//! correctness defects.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::bands;
use crate::rows::{Outcome, Row, RowKey, Table};

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Example {
    pub row: String,
    pub gap: f64,
    pub reachable: bool,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Group {
    pub rows: u64,
    pub perfect: u64,
    pub reachable: u64,
    pub gap: f64,
    pub uncovered: u64,
    /// sum of max(0, T's lower bound - best other)
    pub proven_gap: f64,
    /// rows with a proven gap
    pub proven_rows: u64,
    /// rows with gap > 0 but no proven gap (the bounds overlap)
    pub inconclusive_rows: u64,
    pub examples: Vec<Example>,
}

struct Acc {
    keep: usize,
}

impl Group {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        perfect: bool,
        reachable: bool,
        gap: f64,
        proven: f64,
        uncovered: bool,
        rid: &str,
        acc: &Acc,
    ) {
        self.rows += 1;
        self.proven_gap += proven;
        self.proven_rows += u64::from(proven > 0.0);
        self.inconclusive_rows += u64::from(gap > 0.0 && proven == 0.0);
        self.perfect += u64::from(perfect);
        self.reachable += u64::from(reachable);
        self.gap += gap;
        self.uncovered += u64::from(uncovered);
        if gap > 0.0 || reachable {
            self.examples.push(Example {
                row: rid.to_string(),
                gap,
                reachable,
            });
            if self.examples.len() > 4 * acc.keep {
                sort_examples(&mut self.examples);
                self.examples.truncate(acc.keep);
            }
        }
    }

    fn finish(&mut self, keep: usize) {
        sort_examples(&mut self.examples);
        self.examples.truncate(keep);
    }
}

fn sort_examples(v: &mut [Example]) {
    // larger gap first, reachable before not, then row id
    v.sort_by(|a, b| {
        b.gap
            .total_cmp(&a.gap)
            .then(b.reachable.cmp(&a.reachable))
            .then(a.row.cmp(&b.row))
    });
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Headroom {
    pub tool: String,
    pub tools: Vec<String>,
    pub overall: Group,
    pub by_band: BTreeMap<String, Group>,
    pub by_outcome: BTreeMap<String, Group>,
    pub by_tag: BTreeMap<String, Group>,
    pub common_rows: u64,
    pub tool_ged_on_common: f64,
    pub virtual_best_ged_on_common: f64,
}

pub fn outcome_label(r: &Row) -> &'static str {
    match r.outcome {
        Outcome::Abstain => "abstain",
        Outcome::NoOutput => "no_output",
        Outcome::Unscored => "unscored",
        Outcome::Scored => match r.scored_ged() {
            Some(0.0) => "perfect",
            Some(_) => "imperfect",
            None => "unscored",
        },
    }
}

/// Headroom of `tool` over a table. `tags` overrides the rows' own tags when given.
pub fn compute(
    table: &Table,
    tool: &str,
    tags: Option<&BTreeMap<RowKey, Vec<String>>>,
    keep: usize,
) -> Headroom {
    let acc = Acc { keep };
    let mut overall = Group::default();
    let mut by_band: BTreeMap<String, Group> = BTreeMap::new();
    let mut by_outcome: BTreeMap<String, Group> = BTreeMap::new();
    let mut by_tag: BTreeMap<String, Group> = BTreeMap::new();
    let (mut common, mut tool_common, mut vbest) = (0u64, 0.0f64, 0.0f64);
    let mut tools: Vec<String> = Vec::new();
    for (key, per) in table {
        for t in per.keys() {
            if let Err(pos) = tools.binary_search(t) {
                tools.insert(pos, t.clone());
            }
        }
        let Some(mine) = per.get(tool) else { continue };
        if mine.outcome == Outcome::Abstain {
            continue;
        }
        let best_other = per
            .iter()
            .filter(|(t, _)| t.as_str() != tool)
            .filter_map(|(_, r)| r.scored_ged())
            .min_by(f64::total_cmp);
        let my = mine.scored_ged();
        let perfect = my == Some(0.0);
        let reachable = best_other == Some(0.0) && !perfect;
        let uncovered = my.is_none() && best_other.is_some();
        let (mut gap, mut proven) = (0.0, 0.0);
        if let (Some(m), Some(b)) = (my, best_other) {
            if m > b {
                gap = m - b;
            }
            let lb = mine.ged.as_ref().map_or(0.0, |g| {
                if g.exact {
                    g.value
                } else {
                    g.lower_bound.unwrap_or(0.0)
                }
            });
            proven = (lb - b).max(0.0);
            common += 1;
            tool_common += m;
            vbest += m.min(b);
        }
        let sn = crate::rows::source_graph_of(per).map(|g| g.nodes);
        let rid = key.to_string();
        overall.add(perfect, reachable, gap, proven, uncovered, &rid, &acc);
        by_band
            .entry(bands::band(sn).to_string())
            .or_default()
            .add(perfect, reachable, gap, proven, uncovered, &rid, &acc);
        by_outcome
            .entry(outcome_label(mine).to_string())
            .or_default()
            .add(perfect, reachable, gap, proven, uncovered, &rid, &acc);
        let row_tags = tags.and_then(|t| t.get(key)).unwrap_or(&mine.tags);
        for t in row_tags {
            by_tag
                .entry(t.clone())
                .or_default()
                .add(perfect, reachable, gap, proven, uncovered, &rid, &acc);
        }
    }
    overall.finish(keep);
    for g in by_band
        .values_mut()
        .chain(by_outcome.values_mut())
        .chain(by_tag.values_mut())
    {
        g.finish(keep);
    }
    Headroom {
        tool: tool.to_string(),
        tools,
        overall,
        by_band,
        by_outcome,
        by_tag,
        common_rows: common,
        tool_ged_on_common: tool_common,
        virtual_best_ged_on_common: vbest,
    }
}

/// Rate (hits / rows) and its 95% interval from resampling projects; deterministic for a seed.
pub fn bootstrap_rate(
    per_project: &BTreeMap<String, (u64, u64)>,
    resamples: usize,
    seed: u64,
) -> (f64, f64, f64) {
    let projects: Vec<_> = per_project.values().copied().collect();
    if projects.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let (h, n) = projects
        .iter()
        .fold((0, 0), |(a, b), (h, n)| (a + h, b + n));
    let rate = if n > 0 { h as f64 / n as f64 } else { 0.0 };
    let mut rng = SplitMix64(seed);
    let mut samples = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let (mut sh, mut sn) = (0u64, 0u64);
        for _ in 0..projects.len() {
            let (ph, pn) = projects[(rng.next_u64() % projects.len() as u64) as usize];
            sh += ph;
            sn += pn;
        }
        samples.push(if sn > 0 { sh as f64 / sn as f64 } else { 0.0 });
    }
    samples.sort_by(f64::total_cmp);
    let idx = |q: f64| samples[((q * (resamples - 1) as f64) as usize).min(resamples - 1)];
    (rate, idx(0.025), idx(0.975))
}

/// SplitMix64: tiny, well-known, deterministic PRNG (no dependency, same stream everywhere).
pub struct SplitMix64(pub u64);

impl SplitMix64 {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::GedScore;

    fn key(f: &str) -> RowKey {
        RowKey {
            project: "p".into(),
            arch: "x86_64".into(),
            compiler: "gcc".into(),
            config: "O0".into(),
            binary: "bin".into(),
            function: f.into(),
        }
    }

    fn row(f: &str, tool: &str, outcome: Outcome, ged: Option<f64>, sn: u32) -> Row {
        Row {
            key: key(f),
            tool: tool.into(),
            outcome,
            address: None,
            code_sha256: None,
            ged: ged.map(|value| GedScore {
                value,
                source_nodes: Some(sn),
                ..Default::default()
            }),
            joern_ged: None,
            types: None,
            recompile: None,
            tags: vec![],
            scorer: String::new(),
            failure: None,
            source: None,
            evidence: None,
            source_graph: Some(crate::rows::SourceGraph {
                nodes: sn,
                edges: sn,
                ..Default::default()
            }),
        }
    }

    fn table() -> Table {
        use Outcome::*;
        let rows = vec![
            // another tool perfect, we are not: reachable, gap 4
            row("a", "me", Scored, Some(4.0), 3),
            row("a", "x", Scored, Some(0.0), 3),
            row("a", "y", Scored, Some(9.0), 3),
            // we are best
            row("b", "me", Scored, Some(1.0), 30),
            row("b", "x", Scored, Some(5.0), 30),
            // no output where the other scored
            row("c", "me", NoOutput, None, 60),
            row("c", "x", Scored, Some(2.0), 60),
            // worse, nobody perfect
            row("d", "me", Scored, Some(10.0), 200),
            row("d", "x", Scored, Some(7.0), 200),
            // abstain: not counted
            row("e", "me", Abstain, None, 4),
            row("e", "x", Scored, Some(0.0), 4),
            // only us
            row("f", "me", Scored, Some(0.0), 2),
            // both perfect: nothing to reach
            row("g", "me", Scored, Some(0.0), 3),
            row("g", "x", Scored, Some(0.0), 3),
        ];
        let mut t = Table::new();
        for r in rows {
            t.entry(r.key.clone())
                .or_default()
                .insert(r.tool.clone(), r);
        }
        t
    }

    #[test]
    fn overall_and_common() {
        let h = compute(&table(), "me", None, 5);
        let o = &h.overall;
        assert_eq!((o.rows, o.perfect, o.reachable, o.uncovered), (6, 2, 1, 1));
        assert_eq!(o.gap, 7.0);
        assert_eq!(h.common_rows, 4);
        assert_eq!(h.tool_ged_on_common, 15.0);
        assert_eq!(h.virtual_best_ged_on_common, 8.0);
        assert_eq!(h.tools, ["me", "x", "y"]);
    }

    #[test]
    fn bands_outcomes_tags() {
        let mut tags = BTreeMap::new();
        tags.insert(
            key("a"),
            vec!["extra_goto".to_string(), "over_noded".to_string()],
        );
        tags.insert(key("d"), vec!["over_noded".to_string()]);
        let h = compute(&table(), "me", Some(&tags), 5);
        assert_eq!(
            h.by_band["3-5"].rows + h.by_band[bands::STRAIGHT_LINE].rows,
            3
        );
        assert_eq!(h.by_band[bands::STRAIGHT_LINE].rows, 1);
        assert_eq!(h.by_band["51-100"].uncovered, 1);
        assert_eq!(h.by_band[">100"].gap, 3.0);
        assert_eq!(h.by_outcome["no_output"].rows, 1);
        assert_eq!(h.by_outcome["perfect"].rows, 2);
        let ex: Vec<_> = h.by_tag["over_noded"]
            .examples
            .iter()
            .map(|e| e.row.as_str())
            .collect();
        assert_eq!(ex, ["p/gcc-O0/bin/a", "p/gcc-O0/bin/d"]);
        assert_eq!(
            h.by_tag["over_noded"].examples[0],
            Example {
                row: "p/gcc-O0/bin/a".into(),
                gap: 4.0,
                reachable: true
            }
        );
    }

    #[test]
    fn proven_gaps_use_the_lower_bound() {
        let mut t = Table::new();
        let mut put = |f: &str, tool: &str, value: f64, exact: bool, lb: f64| {
            let mut r = row(f, tool, Outcome::Scored, Some(value), 10);
            let g = r.ged.as_mut().unwrap();
            g.exact = exact;
            g.lower_bound = Some(lb);
            t.entry(r.key.clone()).or_default().insert(tool.into(), r);
        };
        // upper 12, lower 10 against 4: proven 6 of the 8
        put("p", "me", 12.0, false, 10.0);
        put("p", "x", 4.0, true, 4.0);
        // upper 12, lower 3 against 4: the bounds overlap
        put("q", "me", 12.0, false, 3.0);
        put("q", "x", 4.0, true, 4.0);
        // exact 7 against 4: proven 3
        put("r", "me", 7.0, true, 7.0);
        put("r", "x", 4.0, false, 1.0);
        let o = compute(&t, "me", None, 5).overall;
        assert_eq!(o.gap, 8.0 + 8.0 + 3.0);
        assert_eq!(o.proven_gap, 6.0 + 3.0);
        assert_eq!((o.proven_rows, o.inconclusive_rows), (2, 1));
    }

    #[test]
    fn example_ties_by_id() {
        let mut t = Table::new();
        for f in ["f3", "f1", "f2"] {
            for (tool, g) in [("me", 2.0), ("x", 1.0)] {
                let r = row(f, tool, Outcome::Scored, Some(g), 3);
                t.entry(r.key.clone()).or_default().insert(tool.into(), r);
            }
        }
        let h = compute(&t, "me", None, 2);
        let ex: Vec<_> = h.overall.examples.iter().map(|e| e.row.as_str()).collect();
        assert_eq!(ex, ["p/gcc-O0/bin/f1", "p/gcc-O0/bin/f2"]);
    }

    #[test]
    fn bootstrap_is_deterministic_and_bounded() {
        let per: BTreeMap<String, (u64, u64)> = [("a", (5, 10)), ("b", (1, 10)), ("c", (9, 10))]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let r1 = bootstrap_rate(&per, 500, 1);
        assert_eq!(r1, bootstrap_rate(&per, 500, 1));
        let (rate, lo, hi) = r1;
        assert_eq!(rate, 0.5);
        assert!(0.1 <= lo && lo <= rate && rate <= hi && hi <= 0.9);
    }
}

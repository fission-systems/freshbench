//! Static results site: `index.html`, one `gaps-<tool>.html` per tool, and the
//! same numbers as `summary.json`. Generated from result rows only; no number is
//! written by hand. Output is deterministic: no timestamps, sorted keys, fixed
//! float formatting.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::bands;
use crate::headroom::{self, Headroom};
use crate::rows::{Outcome, Table};

/// Provenance shown on every page (from `--meta`): corpus revision, pins, notes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub corpus: String,
    #[serde(default)]
    pub pins: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Results that come from other tracks, shown on the same page.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Extras {
    /// function boundaries (`freshbench boundaries` rows)
    pub boundaries: Option<crate::boundaries::Summary>,
    /// what each tool cost (`timing.jsonl`, boundary rows)
    pub speed: Option<crate::speed::Summary>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Cell {
    pub rows: u64,
    pub perfect: u64,
}

impl Cell {
    pub fn rate(&self) -> f64 {
        if self.rows == 0 {
            0.0
        } else {
            self.perfect as f64 / self.rows as f64
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ToolSummary {
    /// every row of the table (the union of all tools' rows): rows a tool has
    /// no result for count as `missing`, so a run that failed on a hard project
    /// does not shrink that tool's denominator
    pub rows: u64,
    /// rows of the table this tool has no row for (artifact or job missing)
    pub missing: u64,
    /// no-output rows by `Row::failure`
    pub failures: BTreeMap<String, u64>,
    /// scored rows whose output needed tree-sitter error recovery, and how many
    /// of the perfect ones did
    pub parse_recovered: u64,
    pub perfect_parse_recovered: u64,
    /// perfect / scored: the rate on functions the tool printed
    pub perfect_rate_of_scored: f64,
    pub scored: u64,
    pub perfect: u64,
    pub no_output: u64,
    pub unscored: u64,
    pub abstain: u64,
    pub upper_bound_only: u64,
    pub perfect_rate: f64,
    pub perfect_rate_ci95: (f64, f64),
    /// GED 0 on Joern-compatible CFGs
    pub joern_perfect: u64,
    /// signature types right (arity, parameter and return classes)
    pub types_perfect: u64,
    /// rows with type ground truth
    pub types_rows: u64,
    /// local variable classes matched / DWARF locals with a location
    pub locals_coverage: Option<f64>,
    /// perfect on at least one metric family
    pub union_perfect: u64,
    /// rows whose source function has control flow (not `straight-line`),
    /// and the perfect ones among them
    pub branching_rows: u64,
    pub branching_perfect: u64,
    pub branching_perfect_rate: f64,
    /// GED 0 without tree-sitter error recovery inside the function
    pub strict_perfect: u64,
    /// GED 0 on both CFG families
    pub cfg_agreement: u64,
    /// outputs not scored because no line maps to an instruction of the function
    pub no_line_mappings: u64,
    /// output lines with a valid instruction address, and all non-blank lines
    pub mapped_lines: u64,
    pub code_lines: u64,
    /// recompiled to the same instructions (byte match, auxiliary)
    pub recompile_exact: u64,
    /// rows a recompilation was attempted on, and how many compiled
    pub recompile_rows: u64,
    pub recompile_compiles: u64,
    /// perfect on every metric that applies to the row
    pub intersection_perfect: u64,
    pub median_normalized_ged: Option<f64>,
    /// total GED on the rows every tool scored
    pub ged_on_common_rows: f64,
}

/// GED distance from the source: the main structural metric. A row without a
/// score (no output, unparsable output, no result at all) is charged the size
/// of the source graph (nodes + edges), the edits that build it from nothing,
/// so skipping a hard function never lowers a tool's distance. Rows whose
/// source graph no tool reported are left out for every tool.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Distance {
    pub rows: u64,
    pub mean: f64,
    pub mean_lower_bound: f64,
    pub median: f64,
    pub median_lower_bound: f64,
    /// rows at distance 0
    pub at_zero: u64,
    /// rows charged the whole source graph (no score)
    pub charged: u64,
    /// the same over rows whose source has control flow
    pub branching_rows: u64,
    pub branching_mean: f64,
    pub branching_mean_lower_bound: f64,
    pub branching_median: f64,
    pub branching_median_lower_bound: f64,
    pub branching_at_zero: u64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Summary {
    /// tool -> GED distance (FreshBench CFG)
    pub distance: BTreeMap<String, Distance>,
    /// rows no tool scored, so their source size is unknown (out of `distance`)
    pub distance_unmeasured: u64,
    pub tools: Vec<String>,
    pub rows: u64,
    pub common_rows: u64,
    pub overall: BTreeMap<String, ToolSummary>,
    /// dimension -> value -> tool -> cell
    pub by: BTreeMap<String, BTreeMap<String, BTreeMap<String, Cell>>>,
}

#[derive(Default, Clone, Copy)]
struct Pair(u64, u64);

impl std::ops::AddAssign<(u64, u64)> for Pair {
    fn add_assign(&mut self, (a, b): (u64, u64)) {
        self.0 += a;
        self.1 += b;
    }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Fill `out` with every tool's distance; returns the rows left out.
fn distances(
    table: &Table,
    tools: &[String],
    out: &mut BTreeMap<String, Distance>,
    truth: Option<&crate::ground_truth::Index>,
) -> u64 {
    let mut all: BTreeMap<&str, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    let mut lower: BTreeMap<&str, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    let mut unmeasured = 0;
    for (key, per) in table {
        let size = truth.and_then(|index| index.get(key)).map(|t| (t.nodes, t.edges));
        let Some((n, e)) = size.or_else(|| per.values().find_map(|r| {
            let g = r.ged.as_ref()?;
            Some((g.source_nodes?, g.source_edges?))
        })) else {
            unmeasured += 1;
            continue;
        };
        let size = f64::from(n) + f64::from(e);
        let branching = bands::band(Some(n)) != bands::STRAIGHT_LINE;
        for t in tools {
            let d = per.get(t).and_then(|r| r.scored_ged());
            let o = out.entry(t.clone()).or_default();
            o.rows += 1;
            o.charged += u64::from(d.is_none());
            let v = d.unwrap_or(size);
            let lo = if d.is_some() {
                per.get(t).and_then(|r| r.ged.as_ref()).map_or(0.0, |g| {
                    if g.exact {
                        v
                    } else {
                        g.lower_bound.unwrap_or(0.0).clamp(0.0, v)
                    }
                })
            } else {
                size
            };
            let (la, lb) = lower.entry(t.as_str()).or_default();
            la.push(lo);
            if branching {
                lb.push(lo);
            }
            o.at_zero += u64::from(v == 0.0);
            let (a, b) = all.entry(t.as_str()).or_default();
            a.push(v);
            if branching {
                b.push(v);
                o.branching_rows += 1;
                o.branching_at_zero += u64::from(v == 0.0);
            }
        }
    }
    for (t, (mut a, mut b)) in all {
        let o = out.get_mut(t).unwrap();
        let mean = |v: &[f64]| {
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f64>() / v.len() as f64
            }
        };
        o.mean = mean(&a);
        o.branching_mean = mean(&b);
        o.median = median(&mut a);
        o.branching_median = median(&mut b);
        let (mut la, mut lb) = lower.remove(t).unwrap_or_default();
        o.mean_lower_bound = mean(&la);
        o.branching_mean_lower_bound = mean(&lb);
        o.median_lower_bound = median(&mut la);
        o.branching_median_lower_bound = median(&mut lb);
    }
    unmeasured
}

pub fn summarize(table: &Table, resamples: usize) -> Summary {
    summarize_with_truth(table, resamples, None)
}

pub fn summarize_with_truth(
    table: &Table,
    resamples: usize,
    truth: Option<&crate::ground_truth::Index>,
) -> Summary {
    let mut s = Summary::default();
    let mut tools: Vec<String> = table.values().flat_map(|p| p.keys().cloned()).collect();
    tools.sort();
    tools.dedup();
    let mut norm: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut locals: BTreeMap<String, Pair> = BTreeMap::new();
    let mut per_project: BTreeMap<String, BTreeMap<String, (u64, u64)>> = BTreeMap::new();
    s.rows = table.len() as u64;
    for (key, per) in table {
        let common = tools
            .iter()
            .all(|t| per.get(t).and_then(|r| r.scored_ged()).is_some());
        if common {
            s.common_rows += 1;
        }
        let band = truth
            .and_then(|index| index.get(key))
            .map(|t| t.nodes)
            .or_else(|| per.values().find_map(|r| r.ged.as_ref().and_then(|g| g.source_nodes)))
            .map(|n| bands::band(Some(n)))
            .unwrap_or(bands::UNKNOWN);
        let branching = band != bands::STRAIGHT_LINE && band != bands::UNKNOWN;
        for tool in &tools {
            let o = s.overall.entry(tool.clone()).or_default();
            o.rows += 1;
            o.branching_rows += u64::from(branching);
            let Some(r) = per.get(tool) else {
                o.missing += 1;
                per_project
                    .entry(tool.clone())
                    .or_default()
                    .entry(key.project.clone())
                    .or_default()
                    .1 += 1;
                for (dim, val) in [
                    ("arch", key.arch.as_str()),
                    ("compiler", key.compiler.as_str()),
                    ("config", key.config.as_str()),
                    ("band", band),
                ] {
                    s.by.entry(dim.to_string())
                        .or_default()
                        .entry(val.to_string())
                        .or_default()
                        .entry(tool.clone())
                        .or_default()
                        .rows += 1;
                }
                continue;
            };
            if let Some(f) = &r.failure {
                *o.failures.entry(f.clone()).or_default() += 1;
            }
            let recovered = r.outcome == Outcome::Scored
                && r.tags.iter().any(|t| t == crate::dataset::PARSE_RECOVERED);
            o.parse_recovered += u64::from(recovered);
            match r.outcome {
                Outcome::NoOutput => o.no_output += 1,
                Outcome::Unscored => o.unscored += 1,
                Outcome::Abstain => o.abstain += 1,
                Outcome::Scored => o.scored += 1,
            }
            let perfect = r.scored_ged() == Some(0.0);
            o.perfect += u64::from(perfect);
            o.branching_perfect += u64::from(perfect && branching);
            o.perfect_parse_recovered += u64::from(perfect && recovered);
            o.joern_perfect += u64::from(r.scored_joern_ged() == Some(0.0));
            o.union_perfect += u64::from(r.union_perfect());
            o.strict_perfect += u64::from(r.strict_perfect());
            o.cfg_agreement += u64::from(r.cfg_agreement());
            o.no_line_mappings +=
                u64::from(r.tags.iter().any(|t| t == crate::dataset::NO_LINE_MAPPINGS));
            if let Some(e) = &r.evidence {
                o.mapped_lines += u64::from(e.mapped_lines);
                o.code_lines += u64::from(e.code_lines);
            }
            if let Some(rc) = &r.recompile {
                o.recompile_rows += 1;
                o.recompile_compiles += u64::from(rc.compiles);
                o.recompile_exact += u64::from(rc.exact);
            }
            o.intersection_perfect += u64::from(r.intersection_perfect());
            if let Some(t) = &r.types {
                o.types_rows += 1;
                o.types_perfect += u64::from(r.types_perfect());
                *locals.entry(tool.clone()).or_default() +=
                    (u64::from(t.locals_matched), u64::from(t.locals_truth));
            }
            if let Some(g) = &r.ged {
                if r.outcome == Outcome::Scored && !g.exact {
                    o.upper_bound_only += 1;
                }
                if let Some(n) =
                    bands::normalized_ged(r.scored_ged(), g.source_nodes, g.source_edges)
                {
                    norm.entry(tool.clone()).or_default().push(n);
                }
            }
            if common {
                o.ged_on_common_rows += r.scored_ged().unwrap_or(0.0);
            }
            let pp = per_project
                .entry(tool.clone())
                .or_default()
                .entry(key.project.clone())
                .or_default();
            pp.0 += u64::from(perfect);
            pp.1 += 1;
            for (dim, val) in [
                ("arch", key.arch.as_str()),
                ("compiler", key.compiler.as_str()),
                ("config", key.config.as_str()),
                ("band", band),
            ] {
                let c =
                    s.by.entry(dim.to_string())
                        .or_default()
                        .entry(val.to_string())
                        .or_default()
                        .entry(tool.clone())
                        .or_default();
                c.rows += 1;
                c.perfect += u64::from(perfect);
            }
        }
    }
    s.distance_unmeasured = distances(table, &tools, &mut s.distance, truth);
    for (tool, o) in s.overall.iter_mut() {
        o.perfect_rate = if o.rows > 0 {
            o.perfect as f64 / o.rows as f64
        } else {
            0.0
        };
        o.branching_perfect_rate = if o.branching_rows > 0 {
            o.branching_perfect as f64 / o.branching_rows as f64
        } else {
            0.0
        };
        o.perfect_rate_of_scored = if o.scored > 0 {
            o.perfect as f64 / o.scored as f64
        } else {
            0.0
        };
        let (_, lo, hi) = headroom::bootstrap_rate(&per_project[tool], resamples, 0);
        o.perfect_rate_ci95 = (lo, hi);
        if let Some(Pair(m, t)) = locals.get(tool) {
            o.locals_coverage = (*t > 0).then(|| *m as f64 / *t as f64);
        }
        if let Some(v) = norm.get_mut(tool) {
            v.sort_by(f64::total_cmp);
            o.median_normalized_ged = v.get(v.len() / 2).copied();
        }
    }
    s.tools = tools;
    s
}

/// The matched track: only functions present in every build of the table,
/// so a difference between compilers, levels or architectures is not a
/// difference in which functions were measured.
pub fn matched(table: &Table) -> Table {
    use std::collections::{BTreeSet, HashMap};
    type Build<'a> = (&'a str, &'a str, &'a str);
    let builds: BTreeSet<Build> = table
        .keys()
        .map(|k| (k.arch.as_str(), k.compiler.as_str(), k.config.as_str()))
        .collect();
    let mut seen: HashMap<(String, String, String), BTreeSet<Build>> = HashMap::new();
    for (k, per) in table {
        if let Some(r) = per.values().next() {
            seen.entry(r.identity()).or_default().insert((
                k.arch.as_str(),
                k.compiler.as_str(),
                k.config.as_str(),
            ));
        }
    }
    table
        .iter()
        .filter(|(_, per)| {
            per.values()
                .next()
                .is_some_and(|r| seen[&r.identity()].len() == builds.len())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Drop the rows of functions that `freshbench duplicates` found to be copies
/// (every member of a clone group but the first, at `level`). Each report
/// covers one architecture (or all, when it names none); a row of an
/// architecture no report covers is an error, not silently kept.
pub fn dedup(
    table: &Table,
    reports: &[crate::clones::Report],
    level: crate::clones::Level,
) -> anyhow::Result<Table> {
    use std::collections::{BTreeMap, HashSet};
    type Id = (String, String, String);
    let mut drop: BTreeMap<Option<&str>, HashSet<Id>> = BTreeMap::new();
    for report in reports {
        let l = report
            .levels
            .iter()
            .find(|l| l.level == level)
            .ok_or_else(|| anyhow::anyhow!("no {level:?} level in the duplicates report"))?;
        anyhow::ensure!(
            l.top.len() == l.groups,
            "the duplicates report lists {} of {} groups; rerun it with --top 1000000",
            l.top.len(),
            l.groups
        );
        let set = drop.entry(report.arch.as_deref()).or_default();
        for f in l.top.iter().flat_map(|g| g.members.iter().skip(1)) {
            set.insert((f.project.clone(), f.source.clone(), f.name.clone()));
        }
    }
    let mut out = Table::new();
    for (k, per) in table {
        let set = drop
            .get(&Some(k.arch.as_str()))
            .or_else(|| drop.get(&None))
            .ok_or_else(|| anyhow::anyhow!("no duplicates report covers {}", k.arch))?;
        if per
            .values()
            .next()
            .is_none_or(|r| !set.contains(&r.identity()))
        {
            out.insert(k.clone(), per.clone());
        }
    }
    Ok(out)
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn pct(x: f64) -> String {
    format!("{:.1}%", 100.0 * x)
}

fn num(x: u64) -> String {
    let s = x.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

const CSS: &str = "
:root{color-scheme:light dark;--fg:#1d1d1f;--bg:#fff;--mute:#6b6b70;--line:#e3e3e6;--acc:#2f6feb;--warn:#fff4e5}
@media(prefers-color-scheme:dark){:root{--fg:#ececef;--bg:#141416;--mute:#9a9aa2;--line:#2c2c31;--acc:#7aa7ff;--warn:#2b2214}}
body{font:15px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif;color:var(--fg);background:var(--bg);max-width:1100px;margin:2rem auto;padding:0 1rem}
h1{font-size:1.6rem;margin-bottom:.2rem}h2{font-size:1.15rem;margin-top:2.2rem;border-bottom:1px solid var(--line);padding-bottom:.3rem}
p.sub,.mute{color:var(--mute)}a{color:var(--acc)}
.note{background:var(--warn);border-radius:6px;padding:.7rem 1rem;margin:1rem 0}
table{border-collapse:collapse;width:100%;margin:.6rem 0 1rem;font-variant-numeric:tabular-nums}
th,td{padding:.35rem .6rem;border-bottom:1px solid var(--line);text-align:right;white-space:nowrap}
th:first-child,td:first-child{text-align:left}th{font-weight:600;color:var(--mute)}
td.ex{text-align:left;white-space:normal;font-size:.85rem}
code{font-size:.9em}footer{margin-top:3rem;color:var(--mute);font-size:.85rem}
";

fn page(title: &str, meta: &Meta, body: &str) -> String {
    let mut pins = String::new();
    for (k, v) in &meta.pins {
        let _ = write!(
            pins,
            "<tr><td>{}</td><td class=ex><code>{}</code></td></tr>",
            esc(k),
            esc(v)
        );
    }
    let notes: String = meta
        .notes
        .iter()
        .map(|n| format!("<li>{}</li>", esc(n)))
        .collect();
    format!(
        "<!doctype html>\n<html lang=en><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
<title>{t}</title><style>{CSS}</style></head><body>\n\
<h1>{t}</h1><p class=sub>{corpus}</p>\n\
<div class=note><b>What this measures.</b> Main: the control-flow graph edit distance (GED) between each decompiled function and its source, \
both graphs built by FreshBench's one CFG builder (<code>&amp;&amp;</code>, <code>||</code> and <code>?:</code> are control flow), \
and whether a tool finds the functions of a stripped binary by itself (function boundaries). \
Auxiliary: perfect rates on FreshBench's CFG, on Joern's CFG (vendored rust-joern, same GED engine), on types \
(return and parameter classes against DWARF; locals coverage = local classes matched / DWARF locals with a location) and on recompilation \
(byte match: the output, compiled as printed with the binary's own compiler, gives the original instructions). \
<b>Union</b> = perfect on at least one of those (one is enough, so a right signature with a wrong body counts); <b>Intersection</b> = perfect on \
every one that applies; <b>both CFGs</b> = GED 0 on both graph families; <b>strict</b> = GED 0 from output that parsed without error recovery. \
A source without control flow matches any output without control flow, an empty body included, so <b>with control flow</b> columns leave those rows out. \
Only maintained decompilers are shown, and no LLM-based ones (the corpus is public). <b>It does not measure correctness</b>: a function can \
print a wrong constant or argument and still score perfect. Compare tools on the same rows; do not read a score as proof that output is right or wrong.</div>\n\
{body}\n<h2>Provenance</h2><table>{pins}</table><ul>{notes}</ul>\n\
<footer>Generated by <code>freshbench report</code> from result rows. Metric families follow \
<a href=\"https://github.com/Noelo-Lab/decbench\">DecBench</a> (Noelo-Lab); FreshBench uses no DecBench code, so these are not DecBench scores. \
<a href=\"https://github.com/fission-systems/freshbench\">Source and method</a>.</footer>\n</body></html>\n",
        t = esc(title),
        corpus = esc(&meta.corpus),
    )
}

fn dim_table(s: &Summary, dim: &str, order: &[&str]) -> String {
    let Some(m) = s.by.get(dim) else {
        return String::new();
    };
    let mut vals: Vec<&String> = m.keys().collect();
    vals.sort_by_key(|v| {
        (
            order.iter().position(|o| o == v).unwrap_or(usize::MAX),
            (*v).clone(),
        )
    });
    let mut h = String::from("<table><tr><th></th>");
    for t in &s.tools {
        let _ = write!(h, "<th>{}</th>", esc(t));
    }
    h += "<th>rows</th></tr>";
    for v in vals {
        let _ = write!(h, "<tr><td>{}</td>", esc(v));
        let mut rows = 0;
        for t in &s.tools {
            match m[v].get(t) {
                Some(c) => {
                    rows = rows.max(c.rows);
                    let _ = write!(h, "<td>{}</td>", pct(c.rate()));
                }
                None => h += "<td class=mute>-</td>",
            }
        }
        let _ = write!(h, "<td class=mute>{}</td></tr>", num(rows));
    }
    h + "</table>"
}

fn f1(x: Option<&crate::boundaries::Rates>) -> String {
    x.map(|r| format!("{:.3}", r.start_f1))
        .unwrap_or_else(|| "-".into())
}

/// The main metrics: GED distance, function boundaries, and cost.
fn render_main(s: &Summary, x: &Extras) -> String {
    let mut b = String::from(
        "<h2>Main metrics</h2><p class=mute><b>GED distance</b>: edits from the output's control-flow graph to the source's \
(FreshBench CFG), mean, median and rows at 0, over every row; a row without a score is charged the whole source graph, so a tool \
that skips hard functions is not rewarded. Ranges show deterministic GED lower and upper bounds (including fixed penalties), not statistical confidence intervals. \
Read the mean next to the median: a few large functions dominate the mean. Rows are ordered by the mean upper bound; overlapping ranges do not establish a strict rank. \
<b>With control flow</b>: the same over rows whose source has a branch or a loop. \
<b>Function boundaries</b>: start F1 of the functions a tool finds by itself in the stripped binary, as built and with the unwind tables \
removed (they hand out every boundary on x86-64 and AArch64); <b>ends</b>: share of true functions whose end it also found exactly, \
without unwind tables. <b>Cost</b>: CPU milliseconds per function to decompile (the tool's whole process tree). \
<b>Line mappings</b>: share of output lines the tool maps to an instruction of the function, and outputs with no mapped line, which are not \
scored (every tool must say which instructions its lines came from). \
Lower distance and cost, higher F1 and ends are better.</p>",
    );
    b += "<table><tr><th>tool</th><th>GED distance</th><th>with control flow</th><th>start F1 as built</th>\
<th>start F1 no unwind</th><th>ends, no unwind</th><th>cost (CPU ms / fn)</th><th>peak memory</th><th>line mappings</th></tr>";
    let mut order: Vec<&String> = s.tools.iter().collect();
    order.sort_by(|a, b| {
        let d = |t: &String| s.distance.get(t).map_or(f64::MAX, |d| d.mean);
        d(a).total_cmp(&d(b)).then(a.cmp(b))
    });
    for t in order {
        let d = s.distance.get(t).cloned().unwrap_or_default();
        let bnd = |v: &str| {
            x.boundaries
                .as_ref()
                .and_then(|bs| bs.overall.get(&format!("{t} {v}")))
        };
        let sp = x
            .speed
            .as_ref()
            .and_then(|sp| sp.overall.get("decompile"))
            .and_then(|m| m.get(t));
        let _ = write!(
            b,
            "<tr><td><a href=\"gaps-{f}.html\">{n}</a></td>\
<td>{:.1}–{:.1} <span class=mute>med {:.0}–{:.0} · {}/{} at 0</span></td>\
<td>{:.1}–{:.1} <span class=mute>med {:.0}–{:.0} · {}/{} at 0</span></td>\
<td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            d.mean_lower_bound,
            d.mean,
            d.median_lower_bound,
            d.median,
            num(d.at_zero),
            num(d.rows),
            d.branching_mean_lower_bound,
            d.branching_mean,
            d.branching_median_lower_bound,
            d.branching_median,
            num(d.branching_at_zero),
            num(d.branching_rows),
            f1(bnd("as-built")),
            f1(bnd("no-unwind")),
            bnd("no-unwind")
                .map(|r| pct(r.exact_rate))
                .unwrap_or_else(|| "-".into()),
            sp.map(|v| format!("{:.1}", v.cpu_ms_per_function))
                .unwrap_or_else(|| "-".into()),
            sp.map(|v| format!("{:.0} MB", v.max_rss_mb))
                .unwrap_or_else(|| "-".into()),
            s.overall
                .get(t)
                .map(|o| format!(
                    "{} of lines <span class=mute>· {} outputs unmapped</span>",
                    pct(o.mapped_lines as f64 / o.code_lines.max(1) as f64),
                    num(o.no_line_mappings)
                ))
                .unwrap_or_default(),
            f = esc(&file_safe(t)),
            n = esc(t),
        );
    }
    b += "</table>";
    if s.distance_unmeasured > 0 {
        let _ = write!(
            b,
            "<p class=mute>{} rows no tool scored are out of the distance (their source size is unknown).</p>",
            num(s.distance_unmeasured)
        );
    }
    if let Some(sp) = &x.speed {
        b += "<h3>Cost</h3><p class=mute>CPU time is user + system of the tool's whole process tree; wall time depends on \
how many binaries ran at once and on the machine, so compare CPU.</p>\
<table><tr><th>track</th><th>tool</th><th>binaries</th><th>functions</th><th>CPU ms / fn</th><th>median CPU s / binary</th>\
<th>p90</th><th>max</th><th>median memory</th><th>peak memory</th><th>failed calls</th></tr>";
        for (track, tools) in &sp.overall {
            let mut o: Vec<_> = tools.iter().collect();
            o.sort_by(|a, b| {
                a.1.cpu_ms_per_function
                    .total_cmp(&b.1.cpu_ms_per_function)
                    .then(a.0.cmp(b.0))
            });
            for (t, v) in o {
                let _ = write!(
                    b,
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.1}</td><td>{:.1}</td><td>{:.1}</td><td>{:.1}</td><td>{:.0} MB</td><td>{:.0} MB</td><td>{}</td></tr>",
                    esc(track),
                    esc(t),
                    num(v.binaries),
                    num(v.functions),
                    v.cpu_ms_per_function,
                    v.median_cpu_s_per_binary,
                    v.p90_cpu_s_per_binary,
                    v.max_cpu_s_per_binary,
                    v.median_rss_mb,
                    v.max_rss_mb,
                    num(v.failed_calls)
                );
            }
        }
        b += "</table>";
    }
    b
}

pub fn render_index(s: &Summary, meta: &Meta) -> String {
    render_index_with(s, meta, &Extras::default())
}

pub fn render_index_with(s: &Summary, meta: &Meta, x: &Extras) -> String {
    let mut b = render_main(s, x);
    let _ = write!(
        b,
        "<h2>Auxiliary metrics</h2><p class=mute>Perfect rates on each metric family, including recompilation (byte match). \
They are kept for comparison with other benchmarks and to show where a tool is exact; the ranking above does not use them. {} rows, one per (function, build): the same source function appears once per architecture, \
compiler and level, and some functions are copies of others, so rows are not distinct functions (published pages also show every table \
with one function per clone group, under <code>dedup/</code>). A tool's rate is over all rows (no output and a missing result count as not perfect). \
95% interval from resampling projects. {} rows were scored by every tool; GED totals are over those.</p>",
        num(s.rows),
        num(s.common_rows)
    );
    b += "<table><tr><th>tool</th><th>perfect (FreshBench CFG)</th><th>with control flow</th><th>95% interval</th><th>perfect (Joern CFG)</th><th>types perfect</th><th>locals coverage</th><th>Union</th><th>Intersection</th><th>both CFGs</th><th>strict</th>\
<th>recompiles</th><th>byte match</th><th>median normalized GED</th><th>GED on common rows</th><th>no output</th><th>missing</th><th>unscored</th><th>upper bound only</th><th>parse recovered</th></tr>";
    let mut order: Vec<&String> = s.tools.iter().collect();
    order.sort_by(|a, b| {
        s.overall[*b]
            .perfect_rate
            .total_cmp(&s.overall[*a].perfect_rate)
            .then(a.cmp(b))
    });
    for t in order {
        let o = &s.overall[t];
        let _ = write!(
            b,
            "<tr><td><a href=\"gaps-{f}.html\">{n}</a></td><td>{} <span class=mute>({}/{})</span></td><td>{} <span class=mute>({}/{})</span></td><td>{}-{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            pct(o.perfect_rate),
            num(o.perfect),
            num(o.rows),
            pct(o.branching_perfect_rate),
            num(o.branching_perfect),
            num(o.branching_rows),
            pct(o.perfect_rate_ci95.0),
            pct(o.perfect_rate_ci95.1),
            pct(o.joern_perfect as f64 / o.rows.max(1) as f64),
            pct(o.types_perfect as f64 / o.rows.max(1) as f64),
            o.locals_coverage.map(pct).unwrap_or_default(),
            pct(o.union_perfect as f64 / o.rows.max(1) as f64),
            pct(o.intersection_perfect as f64 / o.rows.max(1) as f64),
            pct(o.cfg_agreement as f64 / o.rows.max(1) as f64),
            pct(o.strict_perfect as f64 / o.rows.max(1) as f64),
            if o.recompile_rows > 0 {
                format!(
                    "{} ({}/{})",
                    pct(o.recompile_compiles as f64 / o.recompile_rows as f64),
                    num(o.recompile_compiles),
                    num(o.recompile_rows)
                )
            } else {
                "-".into()
            },
            if o.recompile_rows > 0 {
                pct(o.recompile_exact as f64 / o.recompile_rows as f64)
            } else {
                "-".into()
            },
            o.median_normalized_ged.map(|m| format!("{m:.3}")).unwrap_or_default(),
            num(o.ged_on_common_rows as u64),
            num(o.no_output),
            num(o.missing),
            num(o.unscored),
            num(o.upper_bound_only),
            num(o.parse_recovered),
            f = esc(&file_safe(t)),
            n = esc(t),
        );
    }
    b += "</table>";
    let band_order: Vec<&str> = bands::labels().chain([bands::UNKNOWN]).collect();
    b += "<h2>Perfect rate by function size (source CFG nodes)</h2><p class=mute>GED grows with size; compare tools within a band.</p>";
    b += &dim_table(s, "band", &band_order);
    b += "<h2>By architecture</h2>";
    b += &dim_table(s, "arch", &["x86_64", "i686", "aarch64", "armv7"]);
    b += "<h2>By compiler</h2>";
    b += &dim_table(s, "compiler", &["gcc", "clang", "msvc", "mingw"]);
    b += "<h2>By optimization</h2>";
    b += &dim_table(s, "config", &["O0", "O1", "O2", "O3", "Os", "O2-noinline"]);
    b += "<p>Per tool: where it can improve (rows another tool gets perfect, GED gap to the best other tool) - follow a tool's name above.</p>";
    let title = if meta.title.is_empty() {
        "FreshBench results"
    } else {
        &meta.title
    };
    page(title, meta, &b)
}

pub fn file_safe(tool: &str) -> String {
    tool.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn group_rows(
    title: &str,
    m: &BTreeMap<String, headroom::Group>,
    order: Option<&[&str]>,
) -> String {
    let mut keys: Vec<&String> = m.keys().collect();
    match order {
        Some(o) => keys.sort_by_key(|k| {
            (
                o.iter().position(|x| x == k).unwrap_or(usize::MAX),
                (*k).clone(),
            )
        }),
        None => keys.sort_by(|a, b| m[*b].gap.total_cmp(&m[*a].gap).then(a.cmp(b))),
    }
    let mut h = format!(
        "<h2>{}</h2><table><tr><th></th><th>rows</th><th>perfect</th><th>reachable</th><th>GED gap</th><th>proven gap</th><th>inconclusive rows</th><th>no score</th><th>worst rows (gap)</th></tr>",
        esc(title)
    );
    for k in keys {
        let g = &m[k];
        let ex: Vec<String> = g
            .examples
            .iter()
            .take(3)
            .map(|e| format!("<code>{}</code> ({:.0})", esc(&e.row), e.gap))
            .collect();
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=ex>{}</td></tr>",
            esc(k),
            num(g.rows),
            num(g.perfect),
            num(g.reachable),
            num(g.gap as u64),
            num(g.proven_gap as u64),
            num(g.inconclusive_rows),
            num(g.uncovered),
            ex.join("<br>")
        );
    }
    h + "</table>"
}

pub fn render_gaps(h: &Headroom, meta: &Meta) -> String {
    let o = &h.overall;
    let others: Vec<String> = h.tools.iter().filter(|t| **t != h.tool).cloned().collect();
    let mut b = format!(
        "<p><a href=\"index.html\">All tools</a></p>\
<p><b>{tool}</b> against {others}. <b>Reachable</b>: another decompiler printed the source's control flow exactly (GED 0) \
from the same stripped binary and {tool} did not. <b>GED gap</b>: {tool}'s GED minus the best other tool's on that row; GED from a budget-limited search is an upper bound, \
so the <b>proven gap</b> uses {tool}'s lower bound instead and rows where the bounds overlap are <b>inconclusive</b>. \
These are similarity gaps, not correctness defects; open the output before acting on one.</p>\
<table><tr><th></th><th>value</th></tr>\
<tr><td>rows</td><td>{}</td></tr><tr><td>perfect</td><td>{}</td></tr><tr><td>reachable</td><td>{}</td></tr>\
<tr><td>no score where another tool scored</td><td>{}</td></tr>\
<tr><td>GED on rows both scored ({})</td><td>{}</td></tr><tr><td>per-row best of all tools on those rows</td><td>{}</td></tr>\
<tr><td>gap to the best other tool</td><td>{}</td></tr><tr><td>of which proven (lower bound)</td><td>{}</td></tr>\
<tr><td>rows with an inconclusive gap</td><td>{}</td></tr></table>",
        num(o.rows),
        num(o.perfect),
        num(o.reachable),
        num(o.uncovered),
        num(h.common_rows),
        num(h.tool_ged_on_common as u64),
        num(h.virtual_best_ged_on_common as u64),
        num(o.gap as u64),
        num(o.proven_gap as u64),
        num(o.inconclusive_rows),
        tool = esc(&h.tool),
        others = esc(&others.join(", ")),
    );
    let band_order: Vec<&str> = bands::labels().chain([bands::UNKNOWN]).collect();
    b += &group_rows("By function size", &h.by_band, Some(&band_order));
    b += &group_rows("By outcome", &h.by_outcome, None);
    if !h.by_tag.is_empty() {
        b += "<p class=mute>Shape tags compare what the output prints with the source; a row counts under each of its tags, so tag totals overlap.</p>";
        b += &group_rows("By shape tag", &h.by_tag, None);
    }
    let title = format!(
        "{}: where {} can improve",
        if meta.title.is_empty() {
            "FreshBench"
        } else {
            &meta.title
        },
        h.tool
    );
    page(&title, meta, &b)
}

/// Every file of the site: (relative path, contents).
pub fn site(table: &Table, meta: &Meta, resamples: usize, keep: usize) -> Vec<(String, String)> {
    site_with(table, meta, resamples, keep, &Extras::default())
}

pub fn site_with(
    table: &Table,
    meta: &Meta,
    resamples: usize,
    keep: usize,
    x: &Extras,
) -> Vec<(String, String)> {
    site_with_truth(table, meta, resamples, keep, x, None)
}

pub fn site_with_truth(
    table: &Table,
    meta: &Meta,
    resamples: usize,
    keep: usize,
    x: &Extras,
    truth: Option<&crate::ground_truth::Index>,
) -> Vec<(String, String)> {
    let s = summarize_with_truth(table, resamples, truth);
    let mut out = vec![
        ("index.html".to_string(), render_index_with(&s, meta, x)),
        (
            "summary.json".to_string(),
            serde_json::to_string_pretty(
                &serde_json::json!({"meta": meta, "summary": s, "extras": x}),
            )
            .unwrap()
                + "\n",
        ),
    ];
    for t in &s.tools {
        let h = headroom::compute(table, t, None, keep);
        out.push((format!("gaps-{}.html", file_safe(t)), render_gaps(&h, meta)));
        out.push((
            format!("gaps-{}.json", file_safe(t)),
            serde_json::to_string_pretty(&h).unwrap() + "\n",
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::{GedScore, Row, RowKey};

    fn row(project: &str, arch: &str, f: &str, tool: &str, ged: Option<f64>, sn: u32) -> Row {
        Row {
            key: RowKey {
                project: project.into(),
                arch: arch.into(),
                compiler: "gcc".into(),
                config: "O2".into(),
                binary: "b".into(),
                function: f.into(),
            },
            tool: tool.into(),
            outcome: if ged.is_some() {
                Outcome::Scored
            } else {
                Outcome::NoOutput
            },
            address: None,
            code_sha256: None,
            ged: ged.map(|value| GedScore {
                value,
                exact: true,
                source_nodes: Some(sn),
                source_edges: Some(sn),
                ..Default::default()
            }),
            joern_ged: None,
            types: None,
            recompile: None,
            tags: vec!["over_noded".into()],
            scorer: String::new(),
            failure: None,
            source: None,
            evidence: None,
        }
    }

    fn table() -> Table {
        let mut t = Table::new();
        for r in [
            row("p1", "x86_64", "f", "a", Some(0.0), 3),
            row("p1", "x86_64", "f", "b<x>", Some(2.0), 3),
            row("p2", "armv7", "g", "a", Some(5.0), 30),
            row("p2", "armv7", "g", "b<x>", None, 30),
        ] {
            t.entry(r.key.clone())
                .or_default()
                .insert(r.tool.clone(), r);
        }
        t
    }

    #[test]
    fn summary_numbers() {
        let s = summarize(&table(), 200);
        assert_eq!(s.tools, ["a", "b<x>"]);
        assert_eq!((s.rows, s.common_rows), (2, 1));
        let a = &s.overall["a"];
        assert_eq!((a.rows, a.scored, a.perfect, a.no_output), (2, 2, 1, 0));
        assert_eq!(a.perfect_rate, 0.5);
        assert_eq!(a.ged_on_common_rows, 0.0);
        assert_eq!(s.overall["b<x>"].no_output, 1);
        assert_eq!(s.overall["b<x>"].ged_on_common_rows, 2.0);
        assert_eq!(
            s.by["arch"]["armv7"]["a"],
            Cell {
                rows: 1,
                perfect: 0
            }
        );
        assert_eq!(
            s.by["band"]["3-5"]["a"],
            Cell {
                rows: 1,
                perfect: 1
            }
        );
        assert_eq!(
            s.by["band"]["21-50"]["b<x>"],
            Cell {
                rows: 1,
                perfect: 0
            }
        );
    }

    #[test]
    fn site_is_deterministic_escaped_and_complete() {
        let meta = Meta {
            title: "T".into(),
            corpus: "c & d".into(),
            ..Default::default()
        };
        let a = site(&table(), &meta, 200, 3);
        assert_eq!(a, site(&table(), &meta, 200, 3));
        let names: Vec<_> = a.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "index.html",
                "summary.json",
                "gaps-a.html",
                "gaps-a.json",
                "gaps-b_x_.html",
                "gaps-b_x_.json"
            ]
        );
        let index = &a[0].1;
        assert!(index.contains("b&lt;x&gt;") && !index.contains("b<x>"));
        assert!(index.contains("c &amp; d"));
        assert!(index.contains("It does not measure correctness"));
        assert!(index.contains("50.0%"));
        assert!(a[2].1.contains("reachable"));
    }

    #[test]
    fn a_missing_result_stays_in_the_denominator() {
        // tool c ran only on the easy row (p1/f, perfect) and has no row for
        // p2/g: it must not score 100%
        let mut t = table();
        let r = row("p1", "x86_64", "f", "c", Some(0.0), 3);
        t.get_mut(&r.key).unwrap().insert("c".into(), r);
        let mut nr = row("p2", "armv7", "g", "a", None, 30);
        nr.failure = Some("timeout".into());
        t.get_mut(&nr.key).unwrap().insert("a".into(), nr);
        let s = summarize(&t, 50);
        let c = &s.overall["c"];
        assert_eq!((c.rows, c.missing, c.perfect), (2, 1, 1));
        assert_eq!(c.perfect_rate, 0.5);
        assert_eq!(c.perfect_rate_of_scored, 1.0);
        assert_eq!(
            s.by["arch"]["armv7"]["c"],
            Cell {
                rows: 1,
                perfect: 0
            }
        );
        assert_eq!(s.overall["a"].failures["timeout"], 1);
        assert_eq!(s.overall["a"].missing, 0);
    }

    #[test]
    fn matched_and_dedup_views() {
        let mut t = Table::new();
        let mut put = |f: &str, src: &str, arch: &str| {
            let mut r = row("p", arch, f, "a", Some(0.0), 3);
            r.source = Some(src.into());
            t.entry(r.key.clone()).or_default().insert("a".into(), r);
        };
        put("f", "x.c", "x86_64");
        put("f", "x.c", "armv7");
        put("g", "x.c", "x86_64"); // inlined away on armv7
        put("h", "y.c", "x86_64");
        put("h", "y.c", "armv7");
        let m = matched(&t);
        let names: Vec<_> = m.keys().map(|k| k.function.as_str()).collect();
        assert_eq!(names, ["f", "h", "f", "h"]);
        // h in y.c is a renamed copy of f in x.c
        let f = |name: &str, source: &str| crate::clones::Function {
            project: "p".into(),
            source: source.into(),
            name: name.into(),
            rows: 2,
            tokens: 9,
        };
        let rep = crate::clones::Report {
            arch: None,
            rows: 5,
            functions: 3,
            unreadable: 0,
            levels: vec![crate::clones::LevelReport {
                level: crate::clones::Level::Renamed,
                groups: 1,
                cross_project_groups: 0,
                redundant_functions: 1,
                redundant_rows: 2,
                redundant_functions_50_tokens: 0,
                top: vec![crate::clones::Group {
                    tokens: 9,
                    projects: 1,
                    members: vec![f("f", "x.c"), f("h", "y.c")],
                }],
            }],
            against: vec![],
        };
        let d = dedup(
            &t,
            std::slice::from_ref(&rep),
            crate::clones::Level::Renamed,
        )
        .unwrap();
        assert_eq!(d.len(), 3);
        assert!(d.keys().all(|k| k.function != "h"));
        assert!(dedup(&t, std::slice::from_ref(&rep), crate::clones::Level::Exact).is_err());
        // a report for one architecture does not speak for another
        let mut x86 = rep;
        x86.arch = Some("x86_64".into());
        let e = dedup(
            &t,
            std::slice::from_ref(&x86),
            crate::clones::Level::Renamed,
        );
        assert!(format!("{:#}", e.unwrap_err()).contains("armv7"));
    }

    /// DecBench #6: a perfect on a source without control flow is reported
    /// apart, because an empty output scores it too.
    #[test]
    fn straight_line_sources_are_reported_apart() {
        let mut t = Table::new();
        for r in [
            row("p", "x86_64", "leaf", "a", Some(0.0), 2), // entry -> exit only
            row("p", "x86_64", "loop", "a", Some(3.0), 9),
        ] {
            t.entry(r.key.clone()).or_default().insert("a".into(), r);
        }
        let s = summarize(&t, 50);
        let a = &s.overall["a"];
        assert_eq!((a.rows, a.perfect), (2, 1));
        assert_eq!((a.branching_rows, a.branching_perfect), (1, 0));
        assert_eq!(a.branching_perfect_rate, 0.0);
        assert_eq!(s.by["band"][bands::STRAIGHT_LINE]["a"].perfect, 1);
    }

    #[test]
    fn source_truth_keeps_rows_unscored_by_every_tool_in_distance() {
        let mut t = table();
        let key = row("p3", "x86_64", "missing", "a", None, 3).key;
        t.entry(key.clone()).or_default();
        let truth = [(key.clone(), crate::ground_truth::Entry {
            key,
            source: "f.c".into(),
            source_path: "sources/p/f.c".into(),
            source_sha256: String::new(),
            nodes: 4,
            edges: 5,
        })].into_iter().collect();
        let s = summarize_with_truth(&t, 20, Some(&truth));
        assert_eq!(s.distance_unmeasured, 0);
        assert_eq!(s.distance["a"].rows, 3);
        assert_eq!(s.distance["a"].charged, 1);
        assert_eq!(s.distance["a"].mean, 14.0 / 3.0);
    }

    #[test]
    fn ged_ranges_include_approximate_scores_and_fixed_penalties() {
        let mut t = table();
        for per in t.values_mut() {
            if let Some(g) = per.get_mut("a").and_then(|r| r.ged.as_mut()) {
                if g.value > 0.0 {
                    g.exact = false;
                    g.lower_bound = Some(1.0);
                }
            }
        }
        let s = summarize(&t, 20);
        let a = &s.distance["a"];
        assert_eq!((a.mean_lower_bound, a.mean), (0.5, 2.5));
        assert_eq!((a.median_lower_bound, a.median), (0.5, 2.5));
        let b = &s.distance["b<x>"];
        assert_eq!((b.mean_lower_bound, b.mean), (31.0, 31.0));
        assert_eq!(b.charged, 1);
    }

    #[test]
    fn thousands() {
        assert_eq!(num(0), "0");
        assert_eq!(num(1234567), "1,234,567");
        assert_eq!(num(999), "999");
    }
}

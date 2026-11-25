//! Result rows: one decompiler's result on one function.
//!
//! The v2 row is FreshBench's own JSON contract ([`Row`]). Rows of the v1
//! dataset (DecBench-scored, `ged` + `ged_meta`) are read into the same type so
//! the headroom report can run on both; v1 numbers are DecBench's and are never
//! mixed with v2 numbers in one table.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

/// Architectures of the corpus; `x86_64` builds keep v1's unprefixed names.
pub const ARCHES: [&str; 4] = ["x86_64", "i686", "aarch64", "armv7"];
pub const COMPILERS: [&str; 4] = ["gcc", "clang", "msvc", "mingw"];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RowKey {
    pub project: String,
    pub arch: String,
    pub compiler: String,
    pub config: String,
    pub binary: String,
    pub function: String,
}

impl RowKey {
    /// "gcc-O2" on x86-64, "aarch64-gcc-O2" elsewhere.
    pub fn build(&self) -> String {
        build_name(&self.arch, &self.compiler, &self.config)
    }
}

impl fmt::Display for RowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}/{}",
            self.project,
            self.build(),
            self.binary,
            self.function
        )
    }
}

pub fn build_name(arch: &str, compiler: &str, config: &str) -> String {
    if arch == "x86_64" {
        format!("{compiler}-{config}")
    } else {
        format!("{arch}-{compiler}-{config}")
    }
}

/// Inverse of [`build_name`]: (arch, compiler, config).
pub fn split_build(build: &str) -> Result<(String, String, String)> {
    let mut parts: Vec<&str> = build.split('-').collect();
    let mut arch = "x86_64";
    if let Some(first) = parts.first() {
        if ARCHES.contains(first) {
            arch = first;
            parts.remove(0);
        }
    }
    if parts.len() < 2 || !COMPILERS.contains(&parts[0]) {
        bail!("not a build name: {build}");
    }
    Ok((arch.to_string(), parts[0].to_string(), parts[1..].join("-")))
}

/// What happened when the tool was asked for this function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The tool does not support this target (declared per tool, not inferred).
    Abstain,
    /// The tool ran and printed nothing for the function (crash, timeout, missing).
    NoOutput,
    /// Output exists but could not be scored (it did not parse into a function).
    Unscored,
    /// Scored.
    Scored,
}

/// GED of the output's CFG against the source's, with how it was obtained.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GedScore {
    pub value: f64,
    /// `value` is the exact minimum (else an upper bound >= the true distance).
    #[serde(default)]
    pub exact: bool,
    #[serde(default)]
    pub lower_bound: Option<f64>,
    #[serde(default)]
    pub method: String,
    pub source_nodes: Option<u32>,
    pub source_edges: Option<u32>,
    pub output_nodes: Option<u32>,
    pub output_edges: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    #[serde(flatten)]
    pub key: RowKey,
    pub tool: String,
    pub outcome: Outcome,
    #[serde(default)]
    pub address: Option<u64>,
    #[serde(default)]
    pub code_sha256: Option<String>,
    #[serde(default)]
    pub ged: Option<GedScore>,
    /// GED on Joern-compatible CFGs (vendored rust-joern), same GED engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joern_ged: Option<GedScore>,
    /// Signature and local type classes against DWARF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<crate::types::TypeScore>,
    /// Recompilation against the original code (from `*.recompile.jsonl`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recompile: Option<crate::recompile::RecompileScore>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Scorer that produced the numbers ("freshbench-2.x", or "decbench-v1").
    #[serde(default)]
    pub scorer: String,
    /// Why there is no output (`NoOutput` rows): `adapters::failure_kind`,
    /// `empty`, or `no_record` (the run produced nothing for this row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// the function's source file in the project: with project and function,
    /// the identity of a function across builds (`report --matched/--dedup`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Row {
    /// (project, source file or binary, function): the same function in every build.
    pub fn identity(&self) -> (String, String, String) {
        (
            self.key.project.clone(),
            self.source
                .clone()
                .unwrap_or_else(|| self.key.binary.clone()),
            self.key.function.clone(),
        )
    }
}

impl Row {
    pub fn scored_ged(&self) -> Option<f64> {
        match (self.outcome, &self.ged) {
            (Outcome::Scored, Some(g)) => Some(g.value),
            _ => None,
        }
    }

    pub fn scored_joern_ged(&self) -> Option<f64> {
        match (self.outcome, &self.joern_ged) {
            (Outcome::Scored, Some(g)) => Some(g.value),
            _ => None,
        }
    }

    /// Perfect on at least one metric family.
    pub fn union_perfect(&self) -> bool {
        self.scored_ged() == Some(0.0)
            || self.scored_joern_ged() == Some(0.0)
            || self.types_perfect()
            || self.recompile.as_ref().is_some_and(|r| r.exact)
    }

    /// GED 0 from an output whose function parsed without error recovery.
    pub fn strict_perfect(&self) -> bool {
        self.scored_ged() == Some(0.0) && !self.tags.iter().any(|t| t == "parse_recovered")
    }

    /// GED 0 on both CFG families.
    pub fn cfg_agreement(&self) -> bool {
        self.scored_ged() == Some(0.0) && self.scored_joern_ged() == Some(0.0)
    }

    /// Perfect on every metric that applies to the row: both CFG families,
    /// types where there is DWARF truth, recompilation where it was run.
    pub fn intersection_perfect(&self) -> bool {
        self.cfg_agreement()
            && self.types.as_ref().is_none_or(|t| t.perfect)
            && self.recompile.as_ref().is_none_or(|r| r.exact)
    }

    pub fn types_perfect(&self) -> bool {
        self.outcome == Outcome::Scored && self.types.as_ref().is_some_and(|t| t.perfect)
    }
}

/// A v1 (DecBench-scored) row as published in the v1 dataset.
#[derive(Deserialize)]
struct V1Row {
    project: String,
    compiler: String,
    config: String,
    binary: String,
    function: String,
    tool: String,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default)]
    address: Option<u64>,
    #[serde(default)]
    decompiled: bool,
    #[serde(default)]
    code_sha256: Option<String>,
    #[serde(default)]
    ged: Option<f64>,
    #[serde(default)]
    ged_meta: Option<V1GedMeta>,
}

#[derive(Deserialize, Default)]
struct V1GedMeta {
    source_nodes: Option<u32>,
    source_edges: Option<u32>,
    decompiled_nodes: Option<u32>,
    decompiled_edges: Option<u32>,
    method: Option<String>,
}

impl From<V1Row> for Row {
    fn from(v: V1Row) -> Row {
        let outcome = if !v.decompiled {
            Outcome::NoOutput
        } else if v.ged.is_none() {
            Outcome::Unscored
        } else {
            Outcome::Scored
        };
        let meta = v.ged_meta.unwrap_or_default();
        let ged = v.ged.map(|value| GedScore {
            value,
            exact: false,
            lower_bound: None,
            method: meta.method.clone().unwrap_or_default(),
            source_nodes: meta.source_nodes,
            source_edges: meta.source_edges,
            output_nodes: meta.decompiled_nodes,
            output_edges: meta.decompiled_edges,
        });
        Row {
            key: RowKey {
                project: v.project,
                arch: v.arch.unwrap_or_else(|| "x86_64".into()),
                compiler: v.compiler,
                config: v.config,
                binary: v.binary,
                function: v.function,
            },
            tool: v.tool,
            outcome,
            address: v.address,
            code_sha256: v.code_sha256,
            ged,
            joern_ged: None,
            types: None,
            recompile: None,
            tags: vec![],
            scorer: "decbench-v1".into(),
            failure: None,
            source: None,
        }
    }
}

/// Parse one JSON line as a v2 row, or as a v1 row when it has no `outcome`.
pub fn parse_row(line: &str) -> Result<Row> {
    let v: serde_json::Value = serde_json::from_str(line)?;
    if v.get("outcome").is_some() {
        Ok(serde_json::from_value(v)?)
    } else {
        let r: V1Row = serde_json::from_value(v)?;
        Ok(r.into())
    }
}

pub fn open_lines(path: &Path) -> Result<Box<dyn BufRead>> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    if path.extension().is_some_and(|e| e == "gz") {
        Ok(Box::new(BufReader::with_capacity(
            1 << 20,
            GzDecoder::new(f),
        )))
    } else {
        Ok(Box::new(BufReader::with_capacity(1 << 20, f)))
    }
}

/// Row files under a dataset or run tree, in sorted order.
///
/// Dataset tree: `results/function_results.jsonl` (v1) or `results/rows/*.jsonl[.gz]` (v2).
/// Run tree: every `rows.jsonl` / `rows.jsonl.gz` below the root.
pub fn row_files(root: &Path) -> Result<Vec<PathBuf>> {
    let flat = root.join("results").join("function_results.jsonl");
    if flat.exists() {
        return Ok(vec![flat]);
    }
    let mut out = Vec::new();
    walk_sorted(root, &mut |p| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let under_rows = p
            .parent()
            .and_then(|d| d.file_name())
            .is_some_and(|d| d == "rows");
        if name == "rows.jsonl"
            || name == "rows.jsonl.gz"
            || (under_rows && (name.ends_with(".jsonl") || name.ends_with(".jsonl.gz")))
        {
            out.push(p.to_path_buf());
        }
    })?;
    Ok(out)
}

pub fn walk_sorted(dir: &Path, f: &mut dyn FnMut(&Path)) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("read_dir {}", dir.display()))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let ft = e.file_type()?;
        if ft.is_dir() {
            if !e.file_name().to_string_lossy().starts_with('.') {
                walk_sorted(&p, f)?;
            }
        } else if ft.is_file() {
            f(&p);
        }
    }
    Ok(())
}

fn is_recompile_file(p: &Path) -> bool {
    p.to_string_lossy().ends_with(".recompile.jsonl")
}

/// Call `f` for every row under `root` (recompile side files are not rows).
pub fn for_each_row(root: &Path, mut f: impl FnMut(Row)) -> Result<()> {
    for path in row_files(root)?
        .into_iter()
        .filter(|p| !is_recompile_file(p))
    {
        for (i, line) in open_lines(&path)?.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let row = parse_row(&line).with_context(|| format!("{}:{}", path.display(), i + 1))?;
            f(row);
        }
    }
    Ok(())
}

/// All rows grouped by key, then tool.
pub type Table = BTreeMap<RowKey, BTreeMap<String, Row>>;

/// A line of `<tool>.recompile.jsonl`: which row, and its recompilation score.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecompileLine {
    #[serde(flatten)]
    pub key: RowKey,
    pub tool: String,
    /// sha256 of the output text that was recompiled (the row's `code_sha256`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_sha256: Option<String>,
    pub recompile: crate::recompile::RecompileScore,
}

/// Rows grouped by key and tool, with `*.recompile.jsonl` scores attached.
/// All rows under `root`. Refuses a table that mixes scorers (numbers from
/// different metric definitions are not comparable) and a (row, tool) that
/// occurs twice with different content (two runs of one tool collected
/// together); an identical duplicate is kept once.
pub fn load_table(root: &Path) -> Result<Table> {
    let mut t = Table::new();
    let mut scorers = std::collections::BTreeSet::new();
    let mut conflicts = Vec::new();
    for_each_row(root, |r| {
        scorers.insert(r.scorer.clone());
        let slot = t.entry(r.key.clone()).or_default();
        match slot.get(&r.tool) {
            Some(old) if *old != r => conflicts.push(format!("{} {:?}", r.tool, r.key)),
            Some(_) => {}
            None => {
                slot.insert(r.tool.clone(), r);
            }
        }
    })?;
    if scorers.len() > 1 {
        anyhow::bail!("rows from different scorers: {scorers:?}");
    }
    if !conflicts.is_empty() {
        anyhow::bail!(
            "{} rows occur twice with different content, e.g. {}",
            conflicts.len(),
            conflicts[0]
        );
    }
    let mut stale = Vec::new();
    for path in row_files(root)?
        .into_iter()
        .filter(|p| is_recompile_file(p))
    {
        for line in open_lines(&path)?.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let l: RecompileLine = serde_json::from_str(&line)?;
            let Some(r) = t.get_mut(&l.key).and_then(|m| m.get_mut(&l.tool)) else {
                continue;
            };
            // a recompile score belongs to one output text: an older side file
            // must not attach to a newer decompilation of the same row
            if l.code_sha256.is_some() && l.code_sha256 != r.code_sha256 {
                stale.push(format!("{} {}", l.tool, l.key));
                continue;
            }
            match &r.recompile {
                Some(old) if *old != l.recompile => {
                    anyhow::bail!(
                        "two recompile scores for {} {} ({})",
                        l.tool,
                        l.key,
                        path.display()
                    )
                }
                _ => r.recompile = Some(l.recompile),
            }
        }
    }
    if !stale.is_empty() {
        anyhow::bail!(
            "{} recompile scores are for another output than the row's, e.g. {}",
            stale.len(),
            stale[0]
        );
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_names_roundtrip() {
        assert_eq!(
            build_name("x86_64", "gcc", "O2-noinline"),
            "gcc-O2-noinline"
        );
        assert_eq!(build_name("aarch64", "clang", "Os"), "aarch64-clang-Os");
        assert_eq!(
            split_build("gcc-O2-noinline").unwrap(),
            ("x86_64".into(), "gcc".into(), "O2-noinline".into())
        );
        assert_eq!(
            split_build("armv7-gcc-O0").unwrap(),
            ("armv7".into(), "gcc".into(), "O0".into())
        );
        assert!(split_build("O2").is_err());
        assert!(split_build("aarch64-O2").is_err());
    }

    #[test]
    fn v1_and_v2_rows_parse() {
        let v1 = r#"{"project":"p","compiler":"gcc","config":"O0","binary":"b","function":"f","tool":"t",
            "decompiled":true,"ged":3.0,"ged_meta":{"source_nodes":5,"source_edges":6,"decompiled_nodes":7}}"#;
        let r = parse_row(v1).unwrap();
        assert_eq!(r.outcome, Outcome::Scored);
        assert_eq!(r.key.arch, "x86_64");
        assert_eq!(r.scored_ged(), Some(3.0));
        assert_eq!(r.ged.as_ref().unwrap().output_nodes, Some(7));
        assert_eq!(r.scorer, "decbench-v1");
        let none = r#"{"project":"p","compiler":"gcc","config":"O0","binary":"b","function":"f","tool":"t","decompiled":false}"#;
        assert_eq!(parse_row(none).unwrap().outcome, Outcome::NoOutput);
        let v2 = serde_json::to_string(&r).unwrap();
        assert_eq!(parse_row(&v2).unwrap(), r);
    }

    #[test]
    fn display_is_the_row_id() {
        let k = RowKey {
            project: "p".into(),
            arch: "i686".into(),
            compiler: "gcc".into(),
            config: "O0".into(),
            binary: "b".into(),
            function: "f".into(),
        };
        assert_eq!(k.to_string(), "p/i686-gcc-O0/b/f");
    }

    #[test]
    fn load_table_refuses_mixed_runs() {
        let line = |tool: &str, ged: f64, scorer: &str| {
            let mut r = parse_row(r#"{"project":"p","compiler":"gcc","config":"O0","binary":"b","function":"f","tool":"t","decompiled":true,"ged":1.0}"#).unwrap();
            r.tool = tool.into();
            r.ged.as_mut().unwrap().value = ged;
            r.scorer = scorer.into();
            serde_json::to_string(&r).unwrap() + "\n"
        };
        let d = tempfile::tempdir().unwrap();
        let rows = d.path().join("rows");
        std::fs::create_dir_all(&rows).unwrap();
        // the same row twice (one tool's artifact collected twice): kept once
        std::fs::write(rows.join("a.jsonl"), line("t", 1.0, "s1")).unwrap();
        std::fs::write(rows.join("b.jsonl"), line("t", 1.0, "s1")).unwrap();
        assert_eq!(load_table(d.path()).unwrap().len(), 1);
        // a different result for the same row: two runs mixed
        std::fs::write(rows.join("b.jsonl"), line("t", 2.0, "s1")).unwrap();
        assert!(format!("{:#}", load_table(d.path()).unwrap_err()).contains("twice"));
        // another tool scored by another scorer version
        std::fs::write(rows.join("b.jsonl"), line("u", 2.0, "s2")).unwrap();
        assert!(format!("{:#}", load_table(d.path()).unwrap_err()).contains("scorers"));
    }

    /// A recompile side file from an older decompilation of the same row is refused.
    #[test]
    fn stale_recompile_scores_are_refused() {
        let mut r = parse_row(r#"{"project":"p","compiler":"gcc","config":"O0","binary":"b","function":"f","tool":"t","decompiled":true,"ged":0.0}"#).unwrap();
        r.code_sha256 = Some("new".into());
        let d = tempfile::tempdir().unwrap();
        let rows = d.path().join("rows");
        std::fs::create_dir_all(&rows).unwrap();
        std::fs::write(
            rows.join("t.jsonl"),
            serde_json::to_string(&r).unwrap() + "\n",
        )
        .unwrap();
        let side = |sha: &str, exact: bool| {
            serde_json::to_string(&RecompileLine {
                key: r.key.clone(),
                tool: "t".into(),
                code_sha256: Some(sha.into()),
                recompile: crate::recompile::RecompileScore {
                    compiles: true,
                    exact,
                    ..Default::default()
                },
            })
            .unwrap()
                + "\n"
        };
        std::fs::write(rows.join("t.recompile.jsonl"), side("new", true)).unwrap();
        let t = load_table(d.path()).unwrap();
        assert!(t[&r.key]["t"].recompile.as_ref().unwrap().exact);
        assert!(t[&r.key]["t"].union_perfect() && !t[&r.key]["t"].intersection_perfect());
        // scored for another output text
        std::fs::write(rows.join("t.recompile.jsonl"), side("old", true)).unwrap();
        assert!(format!("{:#}", load_table(d.path()).unwrap_err()).contains("another output"));
        // two different scores for the same output
        std::fs::write(rows.join("t.recompile.jsonl"), side("new", true)).unwrap();
        std::fs::write(rows.join("u.recompile.jsonl"), side("new", false)).unwrap();
        assert!(format!("{:#}", load_table(d.path()).unwrap_err()).contains("two recompile"));
    }
}

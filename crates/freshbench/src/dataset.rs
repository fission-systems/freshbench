//! A dataset tree: targets (functions to decompile), their source TUs, and
//! each tool's decompiled code.
//!
//! Layout (shared by v1 and v2):
//!   configs/full/manifest.json   binaries[] with project, compiler, level, binary, [arch],
//!                                function_info[] {name, address, source_path}
//!   sources/<build>/<project>/<tu>.c          preprocessed translation units
//!   results/<tool>/<build>/<project>/records.jsonl.gz   {binary, function, code}

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::Deserialize;

use crate::rows::{open_lines, split_build, GedScore, Outcome, Row, RowKey};
use crate::{cfg, csyntax, ged, manifest, score, taxonomy};

#[derive(Clone, Debug)]
pub struct Target {
    pub key: RowKey,
    pub address: Option<u64>,
    pub source_path: String,
}

#[derive(Deserialize)]
struct Manifest {
    binaries: Vec<Bin>,
}

#[derive(Deserialize)]
struct Bin {
    project: String,
    compiler: String,
    level: String,
    binary: String,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default)]
    function_info: Vec<FInfo>,
}

#[derive(Deserialize)]
struct FInfo {
    name: String,
    #[serde(default)]
    address: Option<u64>,
    #[serde(default)]
    source_path: Option<String>,
}

pub fn targets(root: &Path) -> Result<Vec<Target>> {
    let p = root.join("configs").join("full").join("manifest.json");
    let m: Manifest = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(&p).with_context(|| format!("open {}", p.display()))?,
    ))?;
    let mut out = BTreeMap::new();
    for b in m.binaries {
        for f in b.function_info {
            let key = RowKey {
                project: b.project.clone(),
                arch: b.arch.clone().unwrap_or_else(|| "x86_64".into()),
                compiler: b.compiler.clone(),
                config: b.level.clone(),
                binary: b.binary.clone(),
                function: f.name,
            };
            if let Some(sp) = f.source_path {
                let sp = resolve_source(root, &key.project, &sp);
                out.insert(
                    key.clone(),
                    Target {
                        key,
                        address: f.address,
                        source_path: sp,
                    },
                );
            }
        }
    }
    Ok(out.into_values().collect())
}

/// The manifest's `sources/<build>/<project>/<tu>.c`, or the deduplicated
/// `sources/<project>/<tu>.c` (TU file names carry their content hash, so the
/// same name is the same text in every build).
fn resolve_source(root: &Path, project: &str, sp: &str) -> String {
    if root.join(sp).exists() {
        return sp.to_string();
    }
    let name = sp.rsplit('/').next().unwrap_or(sp);
    let alt = format!("sources/{project}/{name}");
    if root.join(&alt).exists() {
        alt
    } else {
        sp.to_string()
    }
}

#[derive(Deserialize)]
struct Rec {
    binary: String,
    function: String,
    #[serde(default)]
    code: Option<String>,
}

/// Decompiled code of one tool, keyed by row.
pub fn records(root: &Path, tool: &str) -> Result<HashMap<RowKey, String>> {
    let base = root.join("results").join(tool);
    let mut files = Vec::new();
    for build in sorted_dir(&base)? {
        let Ok((arch, compiler, config)) = split_build(&build) else {
            continue;
        };
        for project in sorted_dir(&base.join(&build))? {
            let f = base.join(&build).join(&project).join("records.jsonl.gz");
            if f.exists() {
                files.push((f, project, arch.clone(), compiler.clone(), config.clone()));
            }
        }
    }
    let parts: Vec<Result<Vec<(RowKey, String)>>> = files
        .par_iter()
        .map(|(f, project, arch, compiler, config)| {
            let mut v = Vec::new();
            for line in open_lines(f)?.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let r: Rec = serde_json::from_str(&line)?;
                let key = RowKey {
                    project: project.clone(),
                    arch: arch.clone(),
                    compiler: compiler.clone(),
                    config: config.clone(),
                    binary: r.binary,
                    function: r.function,
                };
                v.push((key, r.code.unwrap_or_default()));
            }
            Ok(v)
        })
        .collect();
    let mut out = HashMap::new();
    for p in parts {
        out.extend(p?);
    }
    Ok(out)
}

fn sorted_dir(p: &Path) -> Result<Vec<String>> {
    let mut v: Vec<String> = std::fs::read_dir(p)
        .with_context(|| format!("read_dir {}", p.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    Ok(v)
}

/// Score `tool` on every target (or the first `limit` source TUs' targets). Rows sorted by key.
pub fn rescore(root: &Path, tool: &str, limit_tus: Option<usize>, budget: u64) -> Result<Vec<Row>> {
    let targets = targets(root)?;
    let code = records(root, tool)?;
    let mut by_tu: BTreeMap<&str, Vec<&Target>> = BTreeMap::new();
    for t in &targets {
        by_tu.entry(t.source_path.as_str()).or_default().push(t);
    }
    let groups: Vec<_> = by_tu
        .into_iter()
        .take(limit_tus.unwrap_or(usize::MAX))
        .collect();
    let mut rows: Vec<Row> = groups
        .par_iter()
        .flat_map_iter(|(sp, ts)| score_tu(root, sp, ts, &code, tool, budget))
        .collect();
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(rows)
}

fn score_tu(
    root: &Path,
    sp: &str,
    ts: &[&Target],
    code: &HashMap<RowKey, String>,
    tool: &str,
    budget: u64,
) -> Vec<Row> {
    let tu = std::fs::read(root.join(sp))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .ok();
    let tree = tu.as_deref().and_then(|t| csyntax::parse(t).ok());
    let defs = match (&tree, &tu) {
        (Some(tr), Some(t)) => csyntax::definitions(tr, t),
        _ => BTreeMap::new(),
    };
    let mut src_cache: HashMap<&str, Option<(cfg::Graph, csyntax::Features)>> = HashMap::new();
    let mut out = Vec::with_capacity(ts.len());
    for t in ts {
        let mut row = Row {
            key: t.key.clone(),
            tool: tool.to_string(),
            outcome: Outcome::NoOutput,
            address: t.address,
            code_sha256: None,
            ged: None,
            tags: vec![],
            scorer: score::SCORER.to_string(),
        };
        let Some(text) = code.get(&t.key).filter(|c| !c.trim().is_empty()) else {
            out.push(row);
            continue;
        };
        row.code_sha256 = Some(manifest::sha256_bytes(text.as_bytes()));
        let src = src_cache.entry(t.key.function.as_str()).or_insert_with(|| {
            let def = defs.get(&t.key.function).copied().flatten()?;
            let t = tu.as_deref()?;
            Some((cfg::build(def, t), csyntax::features(def, t)))
        });
        let Some((sg, sf)) = src.as_ref() else {
            row.outcome = Outcome::Unscored;
            row.tags = vec!["source_missing".into()];
            out.push(row);
            continue;
        };
        let Ok(ot) = csyntax::parse(text) else {
            row.outcome = Outcome::Unscored;
            out.push(row);
            continue;
        };
        let Some(odef) = csyntax::output_function(&ot, text, &t.key.function) else {
            row.outcome = Outcome::Unscored;
            row.tags = vec!["output_has_no_function".into()];
            out.push(row);
            continue;
        };
        let og = cfg::build(odef, text);
        let of = csyntax::features(odef, text);
        let d = ged::distance_with_budget(sg, &og, budget);
        let (sn, on) = (sg.nodes() as u32, og.nodes() as u32);
        row.outcome = Outcome::Scored;
        row.ged = Some(GedScore {
            value: d.value as f64,
            exact: d.exact,
            lower_bound: Some(d.lower_bound as f64),
            method: d.method.to_string(),
            source_nodes: Some(sn),
            source_edges: Some(sg.edges.len() as u32),
            output_nodes: Some(on),
            output_edges: Some(og.edges.len() as u32),
        });
        row.tags = taxonomy::tags(Some(sn), Some(on), Some(sf), Some(&of));
        out.push(row);
    }
    out
}

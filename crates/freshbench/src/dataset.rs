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
    /// the function's source file in the project (selection identity)
    pub source: String,
    /// unstripped binary, relative to the root (type ground truth)
    pub binary_path: Option<String>,
    /// the build's optimization flags (recompilation)
    pub flags: String,
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
    binary_path: Option<String>,
    #[serde(default)]
    flags: String,
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

/// Targets of a v1 dataset tree (`configs/full/manifest.json`), an assembled
/// corpus (`truth/`) or a prepared tree (`<cfg>/manifest.json`); see `corpus`.
pub fn targets(root: &Path) -> Result<Vec<Target>> {
    let p = root.join("configs").join("full").join("manifest.json");
    if !p.exists() {
        return prepared_targets(root);
    }
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
                        source: sp.clone(),
                        source_path: sp,
                        binary_path: b.binary_path.clone(),
                        flags: b.flags.clone(),
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

fn prepared_targets(root: &Path) -> Result<Vec<Target>> {
    let mut out = BTreeMap::new();
    for u in crate::corpus::units(root)? {
        let pb = &u.targets;
        for (bin, b) in &pb.binaries {
            for f in &b.functions {
                let key = RowKey {
                    project: pb.project.clone(),
                    arch: pb.arch.clone(),
                    compiler: pb.compiler.clone(),
                    config: pb.config.clone(),
                    binary: bin.clone(),
                    function: f.name.clone(),
                };
                out.insert(
                    key.clone(),
                    Target {
                        key,
                        address: Some(f.address),
                        source_path: f.source_path.clone(),
                        source: f.source.clone(),
                        binary_path: Some(format!("{}/{bin}", u.binaries)),
                        flags: pb.flags.clone(),
                    },
                );
            }
        }
    }
    Ok(out.into_values().collect())
}

#[derive(Deserialize)]
struct Rec {
    binary: String,
    function: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    evidence: Option<crate::evidence::Evidence>,
}

/// What a tool's run left for one row: its code (empty when none), the
/// error the adapter recorded, and the tool's line mappings and variables.
pub type Output = (String, Option<String>, Option<crate::evidence::Evidence>);

/// Decompiled code of one tool, keyed by row.
pub fn records(root: &Path, tool: &str) -> Result<HashMap<RowKey, Output>> {
    records_in(&root.join("results").join(tool))
}

/// Records under `base/<build>/<project>/records.jsonl.gz`.
pub fn records_in(base: &Path) -> Result<HashMap<RowKey, Output>> {
    let mut files = Vec::new();
    for build in sorted_dir(base)? {
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
    let parts: Vec<Result<Vec<(RowKey, Output)>>> = files
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
                v.push((key, (r.code.unwrap_or_default(), r.error, r.evidence)));
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

/// What a (function, output) pair scored; cached by content.
#[derive(Clone, Debug, PartialEq, serde::Serialize, Deserialize)]
pub struct Cached {
    pub outcome: Outcome,
    pub ged: Option<GedScore>,
    #[serde(default)]
    pub joern_ged: Option<GedScore>,
    #[serde(default)]
    pub types: Option<crate::types::TypeScore>,
    pub tags: Vec<String>,
}

/// Bumped whenever a metric's definition changes, so cached scores are not reused across definitions.
pub const METRICS_REV: &str = "fbcfg2+joern2+types1";

/// Content-addressed scores: the key is sha256(scorer | metrics revision |
/// budget | function name | source definition fingerprint and DWARF signature |
/// output sha256), so the same source function
/// and the same output text are scored once - across optimization levels,
/// architectures, tools and runs - and always get the same score. Persisted as
/// append-only JSON lines; on load the first value for a key wins.
#[derive(Default)]
pub struct ScoreCache {
    map: std::sync::Mutex<HashMap<String, Cached>>,
    new: std::sync::Mutex<Vec<(String, Cached)>>,
    pub hits: std::sync::atomic::AtomicU64,
    pub misses: std::sync::atomic::AtomicU64,
}

impl ScoreCache {
    pub fn load(path: &Path) -> Result<ScoreCache> {
        let c = ScoreCache::default();
        if path.exists() {
            let mut m = c.map.lock().unwrap();
            for line in open_lines(path)?.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                #[derive(Deserialize)]
                struct L {
                    key: String,
                    value: Cached,
                }
                let l: L = serde_json::from_str(&line)?;
                m.entry(l.key).or_insert(l.value);
            }
        }
        Ok(c)
    }

    pub fn key(budget: u64, function: &str, src_fp: &str, out_sha: &str) -> String {
        Self::key_at(METRICS_REV, budget, function, src_fp, out_sha)
    }

    /// The key under a given metrics revision: a change to a CFG builder, the
    /// vendored rust-joern or the type classes bumps `METRICS_REV`, so no score
    /// from an older definition is ever returned.
    fn key_at(rev: &str, budget: u64, function: &str, src_fp: &str, out_sha: &str) -> String {
        manifest::sha256_bytes(
            format!(
                "{}|{rev}|budget={budget}|{function}|{src_fp}|{out_sha}",
                score::SCORER
            )
            .as_bytes(),
        )
    }

    fn get(&self, k: &str) -> Option<Cached> {
        let v = self.map.lock().unwrap().get(k).cloned();
        let ctr = if v.is_some() {
            &self.hits
        } else {
            &self.misses
        };
        ctr.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        v
    }

    fn put(&self, k: String, v: Cached) {
        let mut m = self.map.lock().unwrap();
        if !m.contains_key(&k) {
            m.insert(k.clone(), v.clone());
            self.new.lock().unwrap().push((k, v));
        }
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append this run's new entries, sorted by key, to `path`.
    pub fn save(&self, path: &Path) -> Result<usize> {
        use std::io::Write;
        let mut new = std::mem::take(&mut *self.new.lock().unwrap());
        new.sort_by(|a, b| a.0.cmp(&b.0));
        let mut f = std::io::BufWriter::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?,
        );
        for (key, value) in &new {
            serde_json::to_writer(&mut f, &serde_json::json!({"key": key, "value": value}))?;
            f.write_all(b"\n")?;
        }
        Ok(new.len())
    }
}

/// Score `tool` on every target (or the first `limit` source TUs' targets). Rows sorted by key.
/// Score `tool`'s records under `root`. A non-empty `builds` (arch-qualified
/// names, `rows::build_name`) keeps only those builds' functions, so a job that
/// decompiled one build scores only that build.
pub fn rescore(
    root: &Path,
    tool: &str,
    limit_tus: Option<usize>,
    budget: u64,
    cache: &ScoreCache,
    builds: &[String],
) -> Result<Vec<Row>> {
    let code = records(root, tool)?;
    rescore_records(root, &code, tool, limit_tus, budget, cache, builds)
}

/// [`rescore`] with the records given (a sweep keeps them outside the corpus).
pub fn rescore_records(
    root: &Path,
    code: &HashMap<RowKey, Output>,
    tool: &str,
    limit_tus: Option<usize>,
    budget: u64,
    cache: &ScoreCache,
    builds: &[String],
) -> Result<Vec<Row>> {
    let mut targets = targets(root)?;
    if !builds.is_empty() {
        targets.retain(|t| {
            let b = crate::rows::build_name(&t.key.arch, &t.key.compiler, &t.key.config);
            builds.contains(&b)
        });
    }
    let mut by_tu: BTreeMap<&str, Vec<&Target>> = BTreeMap::new();
    for t in &targets {
        by_tu.entry(t.source_path.as_str()).or_default().push(t);
    }
    let groups: Vec<_> = by_tu
        .into_iter()
        .take(limit_tus.unwrap_or(usize::MAX))
        .collect();
    let sigs = SignatureCache::default();
    let starts = instruction_starts(root, &targets);
    // Recursive-descent parsers and the CFG builder recurse with the nesting of the
    // text; deeply nested decompiler output overflows a default thread stack.
    let pool = big_stack_pool()?;
    let mut rows: Vec<Row> = pool.install(|| {
        groups
            .par_iter()
            .flat_map_iter(|(sp, ts)| {
                score_tu(root, sp, ts, code, tool, budget, cache, &sigs, &starts)
            })
            .collect()
    });
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(rows)
}

/// (unstripped binary, function low_pc) -> where an instruction of that
/// function may start. Each binary is read once.
pub type StartsMap = HashMap<(String, u64), crate::evidence::Starts>;

pub fn instruction_starts(root: &Path, targets: &[Target]) -> StartsMap {
    let mut by_bin: BTreeMap<&str, Vec<(&str, u64)>> = BTreeMap::new();
    for t in targets {
        if let (Some(b), Some(a)) = (&t.binary_path, t.address) {
            by_bin
                .entry(b.as_str())
                .or_default()
                .push((t.key.arch.as_str(), a));
        }
    }
    by_bin
        .par_iter()
        .flat_map_iter(|(bin, fns)| {
            let data = std::fs::read(root.join(bin)).unwrap_or_default();
            let ranges = crate::dwarf::ranges(&data).unwrap_or_default();
            fns.iter()
                .filter_map(|(arch, low)| {
                    let st =
                        crate::evidence::starts_in(&data, arch, *low, ranges.get(low).copied())?;
                    Some(((bin.to_string(), *low), st))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Tag of an output that is not scored because no line maps to an instruction
/// of the function.
pub const NO_LINE_MAPPINGS: &str = "no_line_mappings";

/// A rayon pool whose threads have a 512 MiB stack (address space, not memory).
pub fn big_stack_pool() -> Result<rayon::ThreadPool> {
    Ok(rayon::ThreadPoolBuilder::new()
        .stack_size(512 << 20)
        .build()?)
}

type SourceFn = (cfg::Graph, csyntax::Features, String);

/// DWARF signatures per unstripped binary, read once.
#[derive(Default)]
pub struct SignatureCache {
    map: std::sync::Mutex<HashMap<String, std::sync::Arc<HashMap<u64, crate::types::Signature>>>>,
}

impl SignatureCache {
    pub fn get(&self, root: &Path, rel: &str, address: u64) -> Option<crate::types::Signature> {
        let existing = self.map.lock().unwrap().get(rel).cloned();
        let m = match existing {
            Some(m) => m,
            None => {
                let m = std::sync::Arc::new(
                    std::fs::read(root.join(rel))
                        .ok()
                        .and_then(|d| crate::dwarf::signatures(&d).ok())
                        .unwrap_or_default(),
                );
                self.map
                    .lock()
                    .unwrap()
                    .entry(rel.to_string())
                    .or_insert(m)
                    .clone()
            }
        };
        m.get(&address).cloned()
    }
}

#[allow(clippy::too_many_arguments)]
fn score_tu(
    root: &Path,
    sp: &str,
    ts: &[&Target],
    code: &HashMap<RowKey, Output>,
    tool: &str,
    budget: u64,
    cache: &ScoreCache,
    sigs: &SignatureCache,
    starts: &StartsMap,
) -> Vec<Row> {
    let tu = std::fs::read(root.join(sp))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .ok();
    let tree = tu.as_deref().and_then(|t| csyntax::parse(t).ok());
    let defs = match (&tree, &tu) {
        (Some(tr), Some(t)) => csyntax::definitions(tr, t),
        _ => BTreeMap::new(),
    };
    let mut src_cache: HashMap<&str, Option<SourceFn>> = HashMap::new();
    let joern_src = tu
        .as_deref()
        .map(crate::joern::source_cfgs)
        .unwrap_or_default();
    let mut out = Vec::with_capacity(ts.len());
    for t in ts {
        let mut row = Row {
            key: t.key.clone(),
            tool: tool.to_string(),
            outcome: Outcome::NoOutput,
            address: t.address,
            code_sha256: None,
            ged: None,
            joern_ged: None,
            types: None,
            recompile: None,
            tags: vec![],
            scorer: score::SCORER.to_string(),
            failure: None,
            source: Some(t.source.clone()),
            evidence: None,
        };
        let Some(text) = code
            .get(&t.key)
            .map(|(c, _, _)| c)
            .filter(|c| !c.trim().is_empty())
        else {
            row.failure = Some(
                match code.get(&t.key) {
                    // the run never produced a record for this row
                    None => "no_record",
                    Some((_, Some(e), _)) => crate::adapters::failure_kind(e),
                    Some((_, None, _)) => "empty",
                }
                .to_string(),
            );
            out.push(row);
            continue;
        };
        let out_sha = manifest::sha256_bytes(text.as_bytes());
        row.code_sha256 = Some(out_sha.clone());
        // the tool's evidence, every address checked against the function's
        // instructions; without one line that maps to an instruction of this
        // function the output is not scored (DecBench's line-mapping contract)
        let mut ev = code.get(&t.key).and_then(|(_, _, e)| e.clone());
        let st = match (&t.binary_path, t.address) {
            (Some(b), Some(a)) => starts.get(&(b.clone(), a)),
            _ => None,
        };
        let checked = match (&mut ev, st) {
            (Some(e), Some(st)) => crate::evidence::validate(e, st),
            _ => crate::evidence::Checked::default(),
        };
        let mut checked = checked;
        checked.code_lines = text.lines().filter(|l| !l.trim().is_empty()).count() as u32;
        row.evidence = Some(checked.clone());
        if checked.mapped_lines == 0 {
            row.outcome = Outcome::Unscored;
            row.tags = vec![NO_LINE_MAPPINGS.into()];
            out.push(row);
            continue;
        }
        let ev = ev.unwrap_or_default();
        let ev_sha =
            manifest::sha256_bytes(serde_json::to_string(&ev).unwrap_or_default().as_bytes());
        let src = src_cache.entry(t.key.function.as_str()).or_insert_with(|| {
            let def = defs.get(&t.key.function).copied().flatten()?;
            let t = tu.as_deref()?;
            Some((
                cfg::build(def, t),
                csyntax::features(def, t),
                crate::prepare::fingerprint(def, t),
            ))
        });
        let Some((sg, sf, src_fp)) = src.as_ref() else {
            row.outcome = Outcome::Unscored;
            row.tags = vec!["source_missing".into()];
            out.push(row);
            continue;
        };
        let truth = match (&t.binary_path, t.address) {
            (Some(b), Some(a)) => sigs.get(root, b, a),
            _ => None,
        };
        let truth_id = truth
            .as_ref()
            .map(|s| {
                manifest::sha256_bytes(serde_json::to_string(s).unwrap_or_default().as_bytes())
            })
            .unwrap_or_default();
        // every input of the score is in the key: the two source graphs
        // themselves (a definition's token fingerprint does not pin the CFG a
        // parser builds from the whole translation unit), the DWARF signature,
        // and the target's data model (`long` is 4 bytes on i686 and ARMv7,
        // so the same output text has another type class there)
        let jg = joern_src.get(&t.key.function).and_then(|g| g.as_ref());
        let dm = crate::types::DataModel::for_arch(&t.key.arch);
        let graph_id = |g: &cfg::Graph| manifest::sha256_bytes(format!("{g:?}").as_bytes());
        let key = ScoreCache::key(
            budget,
            &t.key.function,
            &format!(
                "{src_fp}|{truth_id}|long{}ptr{}ld{}|fb:{}|joern:{}",
                dm.long,
                dm.ptr,
                dm.long_double,
                graph_id(sg),
                jg.map(graph_id).unwrap_or_default()
            ),
            &format!("{out_sha}|{ev_sha}"),
        );
        let c = match cache.get(&key) {
            Some(c) => c,
            None => {
                // a parser or builder panic on one output costs that row only
                let c = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    score_output(
                        sg,
                        sf,
                        jg,
                        truth.as_ref(),
                        dm,
                        text,
                        &t.key.function,
                        budget,
                        &ev,
                    )
                }))
                .unwrap_or_else(|_| Cached {
                    outcome: Outcome::Unscored,
                    ged: None,
                    joern_ged: None,
                    types: None,
                    tags: vec!["scorer_panic".into()],
                });
                cache.put(key, c.clone());
                c
            }
        };
        row.outcome = c.outcome;
        row.ged = c.ged;
        row.joern_ged = c.joern_ged;
        row.types = c.types;
        row.tags = c.tags;
        out.push(row);
    }
    out
}

/// Score one output text against an already built source function.
#[allow(clippy::too_many_arguments)]
pub fn score_output(
    sg: &cfg::Graph,
    sf: &csyntax::Features,
    joern_source: Option<&cfg::Graph>,
    truth: Option<&crate::types::Signature>,
    dm: crate::types::DataModel,
    text: &str,
    function: &str,
    budget: u64,
    ev: &crate::evidence::Evidence,
) -> Cached {
    let unscored = |tag: &str| Cached {
        outcome: Outcome::Unscored,
        ged: None,
        joern_ged: None,
        types: None,
        tags: vec![tag.to_string()],
    };
    let Ok(ot) = csyntax::parse(text) else {
        return unscored("output_unparsable");
    };
    let Some(odef) = csyntax::output_function(&ot, text, function) else {
        return unscored("output_has_no_function");
    };
    // tree-sitter recovered from a syntax error inside the function: the CFG
    // may be built from a partly misread body, so the score is flagged
    let recovered = odef.has_error();
    let og = cfg::build(odef, text);
    let of = csyntax::features(odef, text);
    let d = ged::distance_with_budget(sg, &og, budget);
    let (sn, on) = (sg.nodes() as u32, og.nodes() as u32);
    let joern_ged = joern_source.and_then(|jsg| {
        let jog = crate::joern::output_cfg(text, function)?;
        let jd = ged::distance_with_budget(jsg, &jog, budget);
        Some(GedScore {
            value: jd.value as f64,
            exact: jd.exact,
            lower_bound: Some(jd.lower_bound as f64),
            method: jd.method.to_string(),
            source_nodes: Some(jsg.nodes() as u32),
            source_edges: Some(jsg.edges.len() as u32),
            output_nodes: Some(jog.nodes() as u32),
            output_edges: Some(jog.edges.len() as u32),
        })
    });
    // types come from the tool's evidence only, never from the C text; a tool
    // that reports no variables and no return type has no type score
    let types = truth
        .filter(|_| !ev.variables.is_empty() || ev.return_type.is_some())
        .map(|t| crate::types::compare_evidence(t, ev, dm));
    Cached {
        outcome: Outcome::Scored,
        joern_ged,
        types,
        ged: Some(GedScore {
            value: d.value as f64,
            exact: d.exact,
            lower_bound: Some(d.lower_bound as f64),
            method: d.method.to_string(),
            source_nodes: Some(sn),
            source_edges: Some(sg.edges.len() as u32),
            output_nodes: Some(on),
            output_edges: Some(og.edges.len() as u32),
        }),
        tags: {
            let mut t = taxonomy::tags(Some(sn), Some(on), Some(sf), Some(&of));
            if recovered {
                t.push(PARSE_RECOVERED.into());
            }
            t
        },
    }
}

/// Tag of a score whose output function needed tree-sitter error recovery.
pub const PARSE_RECOVERED: &str = "parse_recovered";

/// Recompile every target `tool` printed and compare it with the original code.
/// `toolchain(arch, compiler, flags)` gives the compiler for a build.
pub fn recompile_all(
    root: &Path,
    tool: &str,
    toolchain: &(dyn Fn(&str, &str, &str) -> crate::recompile::Toolchain + Sync),
    limit: Option<usize>,
) -> Result<Vec<crate::rows::RecompileLine>> {
    let targets = targets(root)?;
    let code = records(root, tool)?;
    let ranges: std::sync::Mutex<HashMap<String, std::sync::Arc<HashMap<u64, u64>>>> =
        Default::default();
    let todo: Vec<&Target> = targets
        .iter()
        .filter(|t| {
            code.get(&t.key)
                .is_some_and(|(c, _, _)| !c.trim().is_empty())
                && t.binary_path.is_some()
                && t.address.is_some()
        })
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    let pool = big_stack_pool()?;
    let mut out: Vec<crate::rows::RecompileLine> = pool.install(|| {
        todo.par_iter()
            .map(|t| {
                let bp = t.binary_path.as_ref().unwrap();
                let existing = ranges.lock().unwrap().get(bp).cloned();
                let r = match existing {
                    Some(r) => r,
                    None => {
                        let r = std::sync::Arc::new(
                            std::fs::read(root.join(bp))
                                .ok()
                                .and_then(|d| crate::dwarf::ranges(&d).ok())
                                .unwrap_or_default(),
                        );
                        ranges
                            .lock()
                            .unwrap()
                            .entry(bp.clone())
                            .or_insert(r)
                            .clone()
                    }
                };
                let low = t.address.unwrap();
                let score = match r.get(&low) {
                    Some(&high) => {
                        let tc = toolchain(&t.key.arch, &t.key.compiler, &t.flags);
                        crate::recompile::score(
                            &tc,
                            &code[&t.key].0,
                            &t.key.function,
                            &root.join(bp),
                            low,
                            high,
                        )
                    }
                    None => crate::recompile::RecompileScore {
                        error: Some("no high_pc in DWARF".into()),
                        ..Default::default()
                    },
                };
                crate::rows::RecompileLine {
                    key: t.key.clone(),
                    tool: tool.to_string(),
                    code_sha256: Some(manifest::sha256_bytes(code[&t.key].0.as_bytes())),
                    recompile: score,
                }
            })
            .collect()
    });
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{write_records, Record};
    use crate::prepare::{PreparedBinary, PreparedBuild, SelectedFunction};
    use std::sync::atomic::Ordering;

    fn tree(root: &Path) {
        let src =
            "int g(int);\nint f(int a, int b)\n{\n  if (a && b) return g(a);\n  return 0;\n}\n";
        std::fs::create_dir_all(root.join("sources/p")).unwrap();
        std::fs::write(root.join("sources/p/0000.f.c"), src).unwrap();
        for (config, code) in [
            ("O2", "int sub_10(int a, int b)\n{\n  if (a == 0) return 0;\n  if (b == 0) return 0;\n  return g(a);\n}\n"),
            ("O3", "int sub_10(int a, int b)\n{\n  if (a == 0) return 0;\n  if (b == 0) return 0;\n  return g(a);\n}\n"),
            ("Os", "int sub_10(int a)\n{\n  return g(a);\n}\n"),
        ] {
            let build = format!("gcc-{config}");
            let pb = PreparedBuild {
                project: "p".into(),
                arch: "x86_64".into(),
                compiler: "gcc".into(),
                config: config.into(),
                flags: String::new(),
                compiler_version: String::new(),
                binaries: [(
                    "bin".to_string(),
                    PreparedBinary {
                        sha256: String::new(),
                        stripped_sha256: String::new(),
                        strip_tool: String::new(),
                        functions: vec![SelectedFunction {
                            name: "f".into(),
                            source: "f.c".into(),
                            address: 0x10,
                            high_pc: None,
                            selection_key: String::new(),
                            thumb: false,
                            tu_sha256: "0000".into(),
                            source_path: "sources/p/0000.f.c".into(),
                        }],
                    },
                )]
                .into(),
            };
            std::fs::create_dir_all(root.join(&build).join("binaries")).unwrap();
            std::fs::write(root.join(&build).join("manifest.json"), serde_json::to_vec(&pb).unwrap()).unwrap();
            // the function's bytes: push rbp; mov eax, 1; ret
            let elf = crate::evidence::test_elf(0x10, &[0x55, 0xb8, 1, 0, 0, 0, 0xc3]);
            std::fs::write(root.join(&build).join("binaries/bin"), elf).unwrap();
            // the tool maps line 3 to the `mov` and line 4 to an address inside it
            let ev = crate::evidence::Evidence {
                line_mappings: vec![
                    crate::evidence::LineMap { line: 3, addresses: vec![0x11] },
                    crate::evidence::LineMap { line: 4, addresses: vec![0x12] },
                ],
                ..Default::default()
            };
            let rec = Record { binary: "bin".into(), function: "f".into(), address: 0x10, code: Some(code.into()), error: None, evidence: Some(ev) };
            write_records(&root.join("results/t").join(&build).join("p/records.jsonl.gz"), &[rec]).unwrap();
        }
    }

    /// The same output for the same source on a 32-bit target is another
    /// score (`long` is 4 bytes there): it must not come from the cache.
    #[test]
    fn cache_does_not_cross_data_models() {
        let d = tempfile::tempdir().unwrap();
        tree(d.path());
        // make the O3 build an i686 build with the same output text
        let m = d.path().join("gcc-O3/manifest.json");
        let mut pb: PreparedBuild = serde_json::from_slice(&std::fs::read(&m).unwrap()).unwrap();
        pb.arch = "i686".into();
        std::fs::write(&m, serde_json::to_vec(&pb).unwrap()).unwrap();
        std::fs::rename(
            d.path().join("results/t/gcc-O3"),
            d.path().join("results/t/i686-gcc-O3"),
        )
        .unwrap();
        let c = ScoreCache::default();
        let rows = rescore(d.path(), "t", None, 1000, &c, &[]).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().filter(|r| r.key.arch == "i686").count(), 1);
        assert_eq!(
            (
                c.hits.load(Ordering::Relaxed),
                c.misses.load(Ordering::Relaxed)
            ),
            (0, 3)
        );
    }

    /// Every output must map at least one line to an instruction of its
    /// function; otherwise it is not scored (DecBench's line-mapping contract).
    #[test]
    fn outputs_without_valid_line_mappings_are_not_scored() {
        let d = tempfile::tempdir().unwrap();
        tree(d.path());
        let rewrite = |config: &str, ev: Option<crate::evidence::Evidence>| {
            let p = d
                .path()
                .join(format!("results/t/gcc-{config}/p/records.jsonl.gz"));
            let mut r: Vec<Record> = {
                use std::io::BufRead;
                std::io::BufReader::new(flate2::read::GzDecoder::new(
                    std::fs::File::open(&p).unwrap(),
                ))
                .lines()
                .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
                .collect()
            };
            r[0].evidence = ev;
            write_records(&p, &r).unwrap();
        };
        // O3: no evidence at all; Os: an address in the middle of an instruction
        rewrite("O3", None);
        rewrite(
            "Os",
            Some(crate::evidence::Evidence {
                line_mappings: vec![crate::evidence::LineMap {
                    line: 2,
                    addresses: vec![0x12, 0x99],
                }],
                ..Default::default()
            }),
        );
        let rows = rescore(d.path(), "t", None, 1000, &ScoreCache::default(), &[]).unwrap();
        let by = |c: &str| rows.iter().find(|r| r.key.config == c).unwrap();
        assert_eq!(by("O2").outcome, Outcome::Scored);
        assert_eq!(by("O2").evidence.as_ref().unwrap().mapped_lines, 1);
        assert_eq!(by("O2").evidence.as_ref().unwrap().dropped_addresses, 1);
        for c in ["O3", "Os"] {
            assert_eq!(by(c).outcome, Outcome::Unscored, "{c}");
            assert_eq!(by(c).tags, [NO_LINE_MAPPINGS]);
            assert!(by(c).code_sha256.is_some());
        }
    }

    #[test]
    fn cache_key_changes_with_the_metrics_revision() {
        let k = ScoreCache::key(200_000, "f", "src", "out");
        assert_eq!(
            k,
            ScoreCache::key_at(METRICS_REV, 200_000, "f", "src", "out")
        );
        assert_ne!(k, ScoreCache::key_at("older", 200_000, "f", "src", "out"));
    }

    #[test]
    fn cache_hits_identical_outputs_and_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        tree(d.path());
        let none = ScoreCache::default();
        let a = rescore(d.path(), "t", None, 1000, &none, &[]).unwrap();
        assert_eq!(a.len(), 3);
        // O2 and O3 printed the same text for the same source: scored once
        assert_eq!(
            (
                none.hits.load(Ordering::Relaxed),
                none.misses.load(Ordering::Relaxed)
            ),
            (1, 2)
        );
        assert_eq!(a[0].ged, a[1].ged);
        // the output duplicates `return 0` into two blocks: insert one node and two
        // edges, delete one edge (worked by hand)
        assert_eq!(a[0].ged.as_ref().unwrap().value, 4.0);
        // persisted cache: a second run scores nothing and gives the same rows
        let p = d.path().join("cache.jsonl");
        assert_eq!(none.save(&p).unwrap(), 2);
        let warm = ScoreCache::load(&p).unwrap();
        let b = rescore(d.path(), "t", None, 1000, &warm, &[]).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            (
                warm.hits.load(Ordering::Relaxed),
                warm.misses.load(Ordering::Relaxed)
            ),
            (3, 0)
        );
        // a different budget is a different key
        let other = ScoreCache::load(&p).unwrap();
        rescore(d.path(), "t", None, 999, &other, &[]).unwrap();
        assert_eq!(other.hits.load(Ordering::Relaxed), 1);
        // a build filter keeps only that build's rows
        let one = rescore(d.path(), "t", None, 1000, &warm, &["gcc-O3".into()]).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].key.config, "O3");
        assert_eq!(one[0], *a.iter().find(|r| r.key.config == "O3").unwrap());
        assert!(
            rescore(d.path(), "t", None, 1000, &warm, &["clang-O3".into()])
                .unwrap()
                .is_empty()
        );
    }
}

//! The quality sweep: one `fission_cli` over a corpus, scored, in a run
//! directory that can be resumed, reported and compared with another run.
//!
//! ```text
//! freshbench sweep run ROOT --cli PATH --repo PATH --label NAME
//! freshbench sweep report runs/NAME
//! freshbench sweep diff runs/A runs/B
//! ```
//!
//! * The tool sees stripped binaries and function starts only.
//! * One checkpoint per binary, written atomically (`bin/<build>/<project>/
//!   <binary>.json`); a run that was interrupted continues from them. A run
//!   directory belongs to one CLI binary, one scorer and one function set: a
//!   resume with another is refused.
//! * A call that crashes or times out is bisected until the failing function
//!   is alone (`adapters::bisect`); the others keep their output.
//! * The scorer is this binary. Its id (`score::SCORER`, a hash of the scoring
//!   sources) is written into the run and every row, and `diff` refuses two
//!   runs scored by different scorers.
//! * `diff` compares output hashes first. Rows whose output text is identical
//!   are not compared on score (and are reported if their scores differ, which
//!   would be a scorer that is not deterministic).
//! * Wall time is recorded per binary, per phase and per session.
//!
//! Nothing is executed from the corpus.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::adapters::{self, CallStats, Record, Tool};
use crate::bands;
use crate::dataset::{self, Output, ScoreCache};
use crate::manifest;
use crate::rows::{Outcome, Row, RowKey};

pub const TOOL: &str = "fission";

#[derive(Clone, Debug)]
pub struct RunArgs {
    pub root: PathBuf,
    pub cli: PathBuf,
    pub repo: PathBuf,
    pub label: String,
    pub runs: PathBuf,
    pub resource_root: Option<PathBuf>,
    pub jobs: usize,
    pub builds: Vec<String>,
    /// substring of `<build>/<project>/<binary>`
    pub only: Option<String>,
    pub binary_timeout_s: u64,
    pub function_timeout_s: u64,
    pub budget: u64,
    pub force: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Git {
    pub commit: String,
    /// tracked files differ from the commit
    pub dirty: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Session {
    pub started_unix: u64,
    pub binaries_decompiled: usize,
    pub binaries_reused: usize,
    pub decompile_wall_s: f64,
    pub score_wall_s: f64,
    pub wall_s: f64,
    pub jobs: usize,
    pub rayon_threads: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    pub label: String,
    pub corpus: String,
    /// sha256 over every selected binary's stripped hash and function starts
    pub corpus_id: String,
    pub binaries: usize,
    pub functions: usize,
    pub cli: String,
    pub cli_sha256: String,
    pub git: Git,
    pub scorer: String,
    pub budget: u64,
    pub builds: Vec<String>,
    pub only: Option<String>,
    pub host: String,
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Checkpoint {
    stripped_sha256: String,
    wall_s: f64,
    stats: CallStats,
    records: Vec<Record>,
}

struct Job {
    build: String,
    project: String,
    arch: String,
    compiler: String,
    config: String,
    binary: String,
    stripped: PathBuf,
    stripped_sha256: String,
    funcs: BTreeMap<u64, String>,
}

impl Job {
    fn id(&self) -> String {
        format!("{}/{}/{}", self.build, self.project, self.binary)
    }
    fn checkpoint(&self, run: &Path) -> PathBuf {
        run.join("bin")
            .join(&self.build)
            .join(&self.project)
            .join(format!("{}.json", self.binary.replace('/', "__")))
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))
}

fn git(repo: &Path) -> Git {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    Git {
        commit: run(&["rev-parse", "HEAD"]).unwrap_or_default(),
        dirty: run(&["status", "--porcelain", "--untracked-files=no"])
            .is_some_and(|s| !s.is_empty()),
    }
}

fn jobs_of(a: &RunArgs) -> Result<Vec<Job>> {
    let mut out = Vec::new();
    for u in crate::corpus::units(&a.root)? {
        if !a.builds.is_empty() && !a.builds.contains(&u.build) {
            continue;
        }
        let pb = &u.targets;
        for (bin, b) in &pb.binaries {
            let j = Job {
                build: u.build.clone(),
                project: pb.project.clone(),
                arch: pb.arch.clone(),
                compiler: pb.compiler.clone(),
                config: pb.config.clone(),
                binary: bin.clone(),
                stripped: a.root.join(&u.stripped).join(bin),
                stripped_sha256: b.stripped_sha256.clone(),
                // Thumb functions are requested as address | 1
                funcs: b
                    .functions
                    .iter()
                    .map(|f| (f.address | u64::from(f.thumb), f.name.clone()))
                    .collect(),
            };
            if a.only.as_ref().is_none_or(|o| j.id().contains(o.as_str())) {
                out.push(j);
            }
        }
    }
    Ok(out)
}

fn corpus_id(jobs: &[Job]) -> String {
    let mut s = String::new();
    for j in jobs {
        s.push_str(&format!("{} {}", j.id(), j.stripped_sha256));
        for (a, n) in &j.funcs {
            s.push_str(&format!(" {a:#x}:{n}"));
        }
        s.push('\n');
    }
    manifest::sha256_bytes(s.as_bytes())
}

pub fn read_meta(run: &Path) -> Result<Meta> {
    let p = run.join("meta.json");
    serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?)
        .with_context(|| format!("parse {}", p.display()))
}

pub fn read_rows(run: &Path) -> Result<BTreeMap<RowKey, Row>> {
    let p = run.join("rows.jsonl");
    let text = std::fs::read_to_string(&p)
        .with_context(|| format!("read {} (the run has not been scored)", p.display()))?;
    let mut out = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let r: Row = serde_json::from_str(line)?;
        out.insert(r.key.clone(), r);
    }
    Ok(out)
}

/// Decompile what is not checkpointed yet, score everything, write the run.
pub fn run(a: &RunArgs) -> Result<PathBuf> {
    let t0 = Instant::now();
    let run = a.runs.join(&a.label);
    std::fs::create_dir_all(&run)?;
    let jobs = jobs_of(a)?;
    if jobs.is_empty() {
        bail!("no binaries selected under {}", a.root.display());
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let n_jobs = if a.jobs == 0 {
        (cores / 2).max(1)
    } else {
        a.jobs
    };
    let rayon_threads = (cores / n_jobs).max(1);
    let mut meta = Meta {
        label: a.label.clone(),
        corpus: a.root.display().to_string(),
        corpus_id: corpus_id(&jobs),
        binaries: jobs.len(),
        functions: jobs.iter().map(|j| j.funcs.len()).sum(),
        cli: a.cli.display().to_string(),
        cli_sha256: manifest::sha256_file(&a.cli)?,
        git: git(&a.repo),
        scorer: crate::score::SCORER.to_string(),
        budget: a.budget,
        builds: a.builds.clone(),
        only: a.only.clone(),
        host: format!(
            "{} {} {cores} cores",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        sessions: vec![],
    };
    if run.join("meta.json").exists() {
        let old = read_meta(&run)?;
        for (what, was, is) in [
            ("fission_cli", &old.cli_sha256, &meta.cli_sha256),
            ("scorer", &old.scorer, &meta.scorer),
            ("function set", &old.corpus_id, &meta.corpus_id),
        ] {
            if was != is {
                bail!(
                    "{} was made with another {what} ({was} then, {is} now); use another --label",
                    run.display()
                );
            }
        }
        meta.sessions = old.sessions;
    }
    let tool = Tool::Fission {
        cli: a.cli.clone(),
        resource_root: Some(
            a.resource_root
                .clone()
                .unwrap_or_else(|| a.repo.join("utils")),
        ),
        function_timeout_ms: a.function_timeout_s * 1000,
        rayon_threads: Some(rayon_threads),
    };
    // largest binaries first, so a long one does not start last
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    let size = |j: &Job| std::fs::metadata(&j.stripped).map_or(0, |m| m.len());
    order.sort_by_key(|&i| std::cmp::Reverse(size(&jobs[i])));
    let todo: Vec<usize> = order
        .into_iter()
        .filter(|&i| {
            a.force
                || !std::fs::read(jobs[i].checkpoint(&run))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Checkpoint>(&b).ok())
                    .is_some_and(|c| c.stripped_sha256 == jobs[i].stripped_sha256)
        })
        .collect();
    eprintln!(
        "sweep {}: {} binaries, {} functions; {} to decompile, {} from checkpoints; {n_jobs} at once x {rayon_threads} threads",
        a.label,
        jobs.len(),
        meta.functions,
        todo.len(),
        jobs.len() - todo.len()
    );
    let t_dec = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(n_jobs)
        .build()?;
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Result<()>> = pool.install(|| {
        todo.par_iter()
            .map(|&i| {
                let j = &jobs[i];
                let t = Instant::now();
                let (records, stats) = adapters::decompile_binary_stats(
                    &tool,
                    &j.stripped,
                    &j.binary,
                    &j.funcs,
                    Duration::from_secs(a.binary_timeout_s),
                    Duration::from_secs(a.function_timeout_s),
                )
                .with_context(|| j.id())?;
                let c = Checkpoint {
                    stripped_sha256: j.stripped_sha256.clone(),
                    wall_s: t.elapsed().as_secs_f64(),
                    stats,
                    records,
                };
                write_atomic(&j.checkpoint(&run), &serde_json::to_vec(&c)?)?;
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n % 50 == 0 || n == todo.len() {
                    eprintln!("  {n}/{} binaries", todo.len());
                }
                Ok(())
            })
            .collect()
    });
    // a binary that could not even be read fails the run after the others are saved
    for r in results {
        r?;
    }
    let decompile_wall_s = t_dec.elapsed().as_secs_f64();

    let t_score = Instant::now();
    let mut code: HashMap<RowKey, Output> = HashMap::new();
    let mut selected: BTreeSet<(String, String, String)> = BTreeSet::new();
    for j in &jobs {
        let c: Checkpoint = serde_json::from_slice(&std::fs::read(j.checkpoint(&run))?)?;
        selected.insert((j.build.clone(), j.project.clone(), j.binary.clone()));
        for r in c.records {
            let key = RowKey {
                project: j.project.clone(),
                arch: j.arch.clone(),
                compiler: j.compiler.clone(),
                config: j.config.clone(),
                binary: r.binary,
                function: r.function,
            };
            code.insert(key, (r.code.unwrap_or_default(), r.error));
        }
    }
    let cache_path = a.runs.join("score-cache.jsonl");
    let cache = ScoreCache::load(&cache_path)?;
    let mut rows =
        dataset::rescore_records(&a.root, &code, TOOL, None, a.budget, &cache, &a.builds)?;
    rows.retain(|r| {
        let b = crate::rows::build_name(&r.key.arch, &r.key.compiler, &r.key.config);
        selected.contains(&(b, r.key.project.clone(), r.key.binary.clone()))
    });
    cache.save(&cache_path)?;
    let mut text = String::new();
    for r in &rows {
        text.push_str(&serde_json::to_string(r)?);
        text.push('\n');
    }
    write_atomic(&run.join("rows.jsonl"), text.as_bytes())?;
    meta.sessions.push(Session {
        started_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
            .saturating_sub(t0.elapsed().as_secs()),
        binaries_decompiled: todo.len(),
        binaries_reused: jobs.len() - todo.len(),
        decompile_wall_s,
        score_wall_s: t_score.elapsed().as_secs_f64(),
        wall_s: t0.elapsed().as_secs_f64(),
        jobs: n_jobs,
        rayon_threads,
    });
    write_atomic(&run.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)?;
    eprintln!(
        "sweep {}: {} rows; decompile {:.1} s, score {:.1} s; {}",
        a.label,
        rows.len(),
        decompile_wall_s,
        t_score.elapsed().as_secs_f64(),
        run.display()
    );
    Ok(run)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Counts {
    pub rows: u64,
    /// GED 0 on the FreshBench CFG
    pub perfect: u64,
    /// rows whose source has control flow, and the perfect ones among them
    pub branching_rows: u64,
    pub branching_perfect: u64,
    pub strict_perfect: u64,
    pub joern_perfect: u64,
    pub types_perfect: u64,
    pub union_perfect: u64,
    pub scored: u64,
    /// sum of GED over scored rows: always read next to `perfect`
    pub distance: f64,
    pub upper_bound_only: u64,
}

impl Counts {
    fn add(&mut self, r: &Row) {
        self.rows += 1;
        let branching = r
            .ged
            .as_ref()
            .and_then(|g| g.source_nodes)
            .is_none_or(|n| bands::band(Some(n)) != bands::STRAIGHT_LINE);
        self.branching_rows += u64::from(branching);
        let perfect = r.scored_ged() == Some(0.0);
        self.perfect += u64::from(perfect);
        self.branching_perfect += u64::from(perfect && branching);
        self.strict_perfect += u64::from(r.strict_perfect());
        self.joern_perfect += u64::from(r.scored_joern_ged() == Some(0.0));
        self.types_perfect += u64::from(r.types_perfect());
        self.union_perfect += u64::from(r.union_perfect());
        if let Some(g) = r.scored_ged() {
            self.scored += 1;
            self.distance += g;
            self.upper_bound_only += u64::from(!r.ged.as_ref().is_some_and(|g| g.exact));
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Timing {
    /// summed over sessions
    pub decompile_wall_s: f64,
    /// the last session's (a resumed run scores everything again, from the cache)
    pub score_wall_s: f64,
    pub sessions: usize,
    pub tool_calls: u64,
    pub failed_calls: u64,
    /// (seconds, `<build>/<project>/<binary>`), slowest first
    pub slowest_binaries: Vec<(f64, String)>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub label: String,
    pub cli_sha256: String,
    pub git: Git,
    pub scorer: String,
    pub corpus_id: String,
    pub binaries: usize,
    pub overall: Counts,
    /// no-output rows by reason
    pub failures: BTreeMap<String, u64>,
    pub unscored: u64,
    /// dimension (arch, compiler, config, band) -> value
    pub by: BTreeMap<String, BTreeMap<String, Counts>>,
    pub timing: Timing,
}

pub fn report(run: &Path) -> Result<Report> {
    let meta = read_meta(run)?;
    let rows = read_rows(run)?;
    let mut rep = Report {
        label: meta.label.clone(),
        cli_sha256: meta.cli_sha256.clone(),
        git: meta.git.clone(),
        scorer: meta.scorer.clone(),
        corpus_id: meta.corpus_id.clone(),
        binaries: meta.binaries,
        ..Default::default()
    };
    for r in rows.values() {
        rep.overall.add(r);
        if r.outcome == Outcome::NoOutput {
            *rep.failures
                .entry(r.failure.clone().unwrap_or_else(|| "unknown".into()))
                .or_default() += 1;
        }
        rep.unscored += u64::from(r.outcome == Outcome::Unscored);
        let band = bands::band(r.ged.as_ref().and_then(|g| g.source_nodes));
        for (dim, val) in [
            ("arch", r.key.arch.as_str()),
            ("compiler", r.key.compiler.as_str()),
            ("config", r.key.config.as_str()),
            ("band", band),
        ] {
            rep.by
                .entry(dim.into())
                .or_default()
                .entry(val.into())
                .or_default()
                .add(r);
        }
    }
    let mut slow = Vec::new();
    let mut files = Vec::new();
    if run.join("bin").is_dir() {
        crate::rows::walk_sorted(&run.join("bin"), &mut |p| files.push(p.to_path_buf()))?;
    }
    for p in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let c: Checkpoint = serde_json::from_slice(&std::fs::read(p)?)?;
        rep.timing.tool_calls += u64::from(c.stats.calls);
        rep.timing.failed_calls += u64::from(c.stats.failed_calls);
        let id = p
            .strip_prefix(run.join("bin"))
            .unwrap_or(p)
            .with_extension("")
            .display()
            .to_string();
        slow.push((c.wall_s, id));
    }
    slow.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    slow.truncate(10);
    rep.timing.slowest_binaries = slow;
    rep.timing.sessions = meta.sessions.len();
    rep.timing.decompile_wall_s = meta.sessions.iter().map(|s| s.decompile_wall_s).sum();
    rep.timing.score_wall_s = meta.sessions.last().map_or(0.0, |s| s.score_wall_s);
    Ok(rep)
}

fn pct(a: u64, b: u64) -> String {
    if b == 0 {
        "-".into()
    } else {
        format!("{:.1}%", 100.0 * a as f64 / b as f64)
    }
}

pub fn render_report(r: &Report) -> String {
    let o = &r.overall;
    let mut s = format!(
        "# sweep {}\n\n- fission_cli sha256 `{}`, commit `{}`{}\n- scorer `{}`, function set `{}`, {} binaries\n\n",
        r.label,
        &r.cli_sha256[..16.min(r.cli_sha256.len())],
        r.git.commit,
        if r.git.dirty { " (dirty)" } else { "" },
        r.scorer,
        &r.corpus_id[..16.min(r.corpus_id.len())],
        r.binaries
    );
    s += "| | rows | perfect | with control flow | strict | Joern perfect | types perfect | distance | scored |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n";
    let line = |name: &str, c: &Counts| {
        format!(
            "| {name} | {} | {} ({}) | {} ({} of {}) | {} | {} | {} | {} | {} |\n",
            c.rows,
            c.perfect,
            pct(c.perfect, c.rows),
            pct(c.branching_perfect, c.branching_rows),
            c.branching_perfect,
            c.branching_rows,
            c.strict_perfect,
            pct(c.joern_perfect, c.rows),
            pct(c.types_perfect, c.rows),
            c.distance as u64,
            c.scored
        )
    };
    s += &line("all", o);
    for dim in ["arch", "compiler", "config", "band"] {
        if let Some(m) = r.by.get(dim) {
            for (k, c) in m {
                s += &line(&format!("{dim} {k}"), c);
            }
        }
    }
    s += &format!(
        "\nDistance is the sum of GED over scored rows; read it next to the perfect count ({} of them are upper bounds).\n",
        o.upper_bound_only
    );
    s += &format!(
        "No output: {} ({}); unscored: {}.\n",
        r.failures.values().sum::<u64>(),
        r.failures
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", "),
        r.unscored
    );
    s += &format!(
        "\nTime: decompile {:.1} s over {} session(s), score {:.1} s; {} tool calls, {} failed (bisected).\n",
        r.timing.decompile_wall_s,
        r.timing.sessions,
        r.timing.score_wall_s,
        r.timing.tool_calls,
        r.timing.failed_calls
    );
    if let Some((t, id)) = r.timing.slowest_binaries.first() {
        s += &format!("Slowest binary: {id} ({t:.1} s).\n");
    }
    s
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Change {
    pub row: String,
    pub ged_a: Option<f64>,
    pub ged_b: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Diff {
    pub a: String,
    pub b: String,
    pub same_function_set: bool,
    pub only_a: usize,
    pub only_b: usize,
    pub common: usize,
    /// identical output text (same sha256), or no output on both sides
    pub identical_outputs: usize,
    pub changed_outputs: usize,
    /// identical output but different score: must be 0 for a deterministic scorer
    pub identical_output_different_score: usize,
    pub gained_output: usize,
    pub lost_output: usize,
    pub perfect_gained: usize,
    pub perfect_lost: usize,
    pub branching_perfect_gained: usize,
    pub branching_perfect_lost: usize,
    pub joern_perfect_gained: usize,
    pub joern_perfect_lost: usize,
    pub types_perfect_gained: usize,
    pub types_perfect_lost: usize,
    /// sum of (GED b - GED a) over changed rows scored on both sides
    pub distance_delta: f64,
    pub better: usize,
    pub worse: usize,
    /// largest GED increases and decreases
    pub regressions: Vec<Change>,
    pub improvements: Vec<Change>,
    pub decompile_wall_a: f64,
    pub decompile_wall_b: f64,
}

/// Compare two runs: outputs first, scores only where the output changed.
pub fn diff(run_a: &Path, run_b: &Path, keep: usize) -> Result<Diff> {
    let (ma, mb) = (read_meta(run_a)?, read_meta(run_b)?);
    if ma.scorer != mb.scorer {
        bail!(
            "the runs were scored by different scorers ({} and {}): rescore one of them first",
            ma.scorer,
            mb.scorer
        );
    }
    let (ra, rb) = (read_rows(run_a)?, read_rows(run_b)?);
    let mut d = Diff {
        a: ma.label.clone(),
        b: mb.label.clone(),
        same_function_set: ma.corpus_id == mb.corpus_id,
        only_a: ra.keys().filter(|k| !rb.contains_key(*k)).count(),
        only_b: rb.keys().filter(|k| !ra.contains_key(*k)).count(),
        decompile_wall_a: ma.sessions.iter().map(|s| s.decompile_wall_s).sum(),
        decompile_wall_b: mb.sessions.iter().map(|s| s.decompile_wall_s).sum(),
        ..Default::default()
    };
    let branching = |r: &Row| {
        r.ged
            .as_ref()
            .and_then(|g| g.source_nodes)
            .is_none_or(|n| bands::band(Some(n)) != bands::STRAIGHT_LINE)
    };
    let mut changes: Vec<(f64, Change)> = Vec::new();
    for (k, a) in &ra {
        let Some(b) = rb.get(k) else { continue };
        d.common += 1;
        if a.code_sha256 == b.code_sha256 {
            d.identical_outputs += 1;
            let mut x = a.clone();
            x.recompile = b.recompile.clone();
            d.identical_output_different_score += usize::from(x != *b);
            continue;
        }
        d.changed_outputs += 1;
        d.gained_output += usize::from(a.code_sha256.is_none());
        d.lost_output += usize::from(b.code_sha256.is_none());
        let flip = |pa: bool, pb: bool, gained: &mut usize, lost: &mut usize| {
            *gained += usize::from(!pa && pb);
            *lost += usize::from(pa && !pb);
        };
        let (pa, pb) = (a.scored_ged() == Some(0.0), b.scored_ged() == Some(0.0));
        flip(pa, pb, &mut d.perfect_gained, &mut d.perfect_lost);
        // one source function: either row tells whether it has control flow
        let br = branching(a) && branching(b);
        flip(
            pa && br,
            pb && br,
            &mut d.branching_perfect_gained,
            &mut d.branching_perfect_lost,
        );
        flip(
            a.scored_joern_ged() == Some(0.0),
            b.scored_joern_ged() == Some(0.0),
            &mut d.joern_perfect_gained,
            &mut d.joern_perfect_lost,
        );
        flip(
            a.types_perfect(),
            b.types_perfect(),
            &mut d.types_perfect_gained,
            &mut d.types_perfect_lost,
        );
        if let (Some(ga), Some(gb)) = (a.scored_ged(), b.scored_ged()) {
            d.distance_delta += gb - ga;
            d.better += usize::from(gb < ga);
            d.worse += usize::from(gb > ga);
            if gb != ga {
                changes.push((
                    gb - ga,
                    Change {
                        row: k.to_string(),
                        ged_a: Some(ga),
                        ged_b: Some(gb),
                    },
                ));
            }
        }
    }
    changes.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.row.cmp(&y.1.row)));
    d.regressions = changes
        .iter()
        .filter(|c| c.0 > 0.0)
        .take(keep)
        .map(|c| c.1.clone())
        .collect();
    d.improvements = changes
        .iter()
        .rev()
        .filter(|c| c.0 < 0.0)
        .take(keep)
        .map(|c| c.1.clone())
        .collect();
    Ok(d)
}

pub fn render_diff(d: &Diff) -> String {
    let mut s = format!("# sweep diff: {} -> {}\n\n", d.a, d.b);
    if !d.same_function_set {
        s += &format!(
            "The runs cover different function sets ({} rows only in {}, {} only in {}); the {} common rows are compared.\n\n",
            d.only_a, d.a, d.only_b, d.b, d.common
        );
    }
    s += &format!(
        "Outputs: {} of {} rows identical, {} changed ({} gained an output, {} lost theirs).\n",
        d.identical_outputs, d.common, d.changed_outputs, d.gained_output, d.lost_output
    );
    if d.identical_output_different_score > 0 {
        s += &format!(
            "WARNING: {} rows have identical output and different scores: the scorer is not deterministic.\n",
            d.identical_output_different_score
        );
    }
    if d.changed_outputs == 0 {
        s += "No output changed: the scores are the same.\n";
    } else {
        s += &format!(
            "\nOn the {} changed rows:\n\n| | gained | lost | net |\n|---|---:|---:|---:|\n",
            d.changed_outputs
        );
        for (name, g, l) in [
            ("perfect (FreshBench CFG)", d.perfect_gained, d.perfect_lost),
            (
                "perfect, with control flow",
                d.branching_perfect_gained,
                d.branching_perfect_lost,
            ),
            (
                "Joern perfect",
                d.joern_perfect_gained,
                d.joern_perfect_lost,
            ),
            (
                "types perfect",
                d.types_perfect_gained,
                d.types_perfect_lost,
            ),
        ] {
            s += &format!("| {name} | {g} | {l} | {:+} |\n", g as i64 - l as i64);
        }
        s += &format!(
            "\nDistance (sum of GED where both scored): {:+}; {} rows closer, {} farther.\n",
            d.distance_delta as i64, d.better, d.worse
        );
        for (title, list) in [("Farther", &d.regressions), ("Closer", &d.improvements)] {
            if !list.is_empty() {
                s += &format!("\n{title}:\n");
                for c in list {
                    s += &format!(
                        "- `{}` {} -> {}\n",
                        c.row,
                        c.ged_a.unwrap_or(0.0),
                        c.ged_b.unwrap_or(0.0)
                    );
                }
            }
        }
    }
    s += &format!(
        "\nDecompile time: {:.1} s -> {:.1} s.\n",
        d.decompile_wall_a, d.decompile_wall_b
    );
    s
}

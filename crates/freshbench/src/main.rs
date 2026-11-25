use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use freshbench::{bands, callsites, cfg, csyntax, headroom, manifest, rows, score};

#[cfg(feature = "sentry")]
mod telemetry;

#[derive(Parser)]
#[command(
    name = "freshbench",
    version,
    about = "FreshBench: decompiler benchmark with its own ground truth, metrics and improvement targets"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Score one decompiled function against its source function (JSON).
    Score {
        /// Preprocessed translation unit holding the source definition.
        #[arg(long)]
        source: PathBuf,
        /// Decompiler output for the function.
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        function: String,
    },
    /// Print the CFG FreshBench builds for a function in a C file (JSON).
    Cfg {
        file: PathBuf,
        #[arg(long)]
        function: String,
    },
    /// Score a tool's decompiled code in a dataset tree with FreshBench's metrics (native rows, JSON lines).
    Rescore {
        root: PathBuf,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        out: PathBuf,
        /// Only the first N source TUs (in path order).
        #[arg(long)]
        limit_tus: Option<usize>,
        #[arg(long, default_value_t = freshbench::ged::DEFAULT_BUDGET)]
        budget: u64,
        /// content-addressed score cache (JSON lines; created if missing, new entries appended)
        #[arg(long)]
        cache: Option<PathBuf>,
        /// only these arch-qualified builds (`gcc-O0`, `aarch64-gcc-O0`; comma list)
        #[arg(long, value_delimiter = ',')]
        builds: Vec<String>,
    },
    /// Eligible functions of one build directory (build.json, tu_log.jsonl, i/, bin/).
    Eligible {
        build_dir: PathBuf,
        #[arg(long)]
        project: String,
        #[arg(long, value_delimiter = ',')]
        submodules: Vec<String>,
        /// JSON list of token fingerprints to exclude
        #[arg(long)]
        exclude: Option<PathBuf>,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Select rows for every build of a project, strip, and write the prepared tree.
    Prepare {
        #[arg(long)]
        project: String,
        /// directory holding <compiler>-<config>/ build outputs
        #[arg(long)]
        builds: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, value_delimiter = ',')]
        submodules: Vec<String>,
        #[arg(long)]
        exclude: Option<PathBuf>,
        #[arg(long, default_value_t = 300)]
        select: usize,
        #[arg(long, default_value = "/opt/llvm/bin/llvm-strip")]
        strip_tool: PathBuf,
        /// comma list of <compiler>-<config> builds
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// count eligible functions only (qualification pre-check), write nothing but the summary
        #[arg(long)]
        count_only: bool,
    },
    /// Token fingerprints of every function defined in the C files under a directory
    /// (to exclude another corpus's functions from selection). Sorted JSON list.
    Fingerprints {
        dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Source-level clone audit of the selected functions (exact copies,
    /// consistently renamed copies, and copies with other literals); JSON.
    Duplicates {
        root: PathBuf,
        /// one architecture only
        #[arg(long)]
        arch: Option<String>,
        /// groups listed per level
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// also count selected functions defined in this corpus's .c/.i files
        #[arg(long)]
        against: Option<PathBuf>,
    },
    /// Run a decompiler on every prepared build under ROOT (stripped binaries,
    /// selected addresses); writes ROOT/results/<tool>/<build>/<project>/records.jsonl.gz.
    Decompile {
        root: PathBuf,
        /// fission, or any name with --adapter
        #[arg(long)]
        tool: String,
        /// fission_cli path (tool fission)
        #[arg(long)]
        fission_cli: Option<PathBuf>,
        #[arg(long)]
        resource_root: Option<PathBuf>,
        /// adapter command following the script contract (other tools)
        #[arg(long, num_args = 1.., allow_hyphen_values = true)]
        adapter: Vec<String>,
        #[arg(long, default_value_t = 1800)]
        binary_timeout_s: u64,
        #[arg(long, default_value_t = 300)]
        function_timeout_s: u64,
        /// only these arch-qualified builds (`gcc-O0`, `aarch64-gcc-O0`; comma list)
        #[arg(long, value_delimiter = ',')]
        builds: Vec<String>,
        /// binaries decompiled at once (each one a separate tool process)
        #[arg(long, default_value_t = 1)]
        jobs: usize,
    },
    /// Function boundary track: each tool lists the functions it finds in every
    /// stripped binary under ROOT (no addresses given); compared with the
    /// unstripped binary's symbols and DWARF. Writes one JSON line per binary.
    Boundaries {
        root: PathBuf,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        fission_cli: Option<PathBuf>,
        #[arg(long)]
        resource_root: Option<PathBuf>,
        #[arg(long, num_args = 1.., allow_hyphen_values = true)]
        adapter: Vec<String>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 1800)]
        binary_timeout_s: u64,
        #[arg(long, value_delimiter = ',')]
        builds: Vec<String>,
        #[arg(long, default_value_t = 1)]
        jobs: usize,
        /// as-built (stripped binary as shipped) or no-unwind (unwind tables removed)
        #[arg(long, default_value = "as-built")]
        variant: String,
    },
    /// Rates per tool (overall, distinct binaries, by arch/compiler/level) from
    /// `boundaries` rows (files or directories of *.jsonl); JSON.
    BoundariesSummary { rows: Vec<PathBuf> },
    /// Record or check where results came from (CI provenance and page metadata).
    Provenance {
        #[command(subcommand)]
        action: ProvCmd,
    },
    /// Upper bound on the eligible functions of one build directory (fast pre-filter).
    Quickcount { build_dir: PathBuf },
    /// Set key=value pairs in a JSON status file.
    Status { file: PathBuf, pairs: Vec<String> },
    /// How fast each tool is: CPU time, wall time and peak memory per binary and
    /// per function, from `timing.jsonl` files (`decompile`) and boundary rows.
    Speed {
        /// files or directories searched for *.jsonl
        paths: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// The Fission quality sweep: decompile a corpus with one fission_cli, score
    /// it, and keep a run directory that can be resumed, reported and compared.
    Sweep {
        #[command(subcommand)]
        action: SweepCmd,
    },
    /// Merge a prepared tree (`prepare --out`) into a corpus root with
    /// arch-qualified builds: binaries/ stripped/ truth/ sources/<arch>/ projects/.
    Assemble {
        prepared: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// the project's license files (copied to projects/<project>/)
        #[arg(long)]
        license: Option<PathBuf>,
    },
    /// Static results site (index.html, gaps-<tool>.html, summary.json) from result rows.
    Report {
        root: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// JSON {title, corpus, pins: {..}, notes: [..]} shown on every page
        #[arg(long)]
        meta: Option<PathBuf>,
        #[arg(long, default_value_t = 2000)]
        resamples: usize,
        #[arg(long, default_value_t = 5)]
        keep: usize,
        /// only functions present in every build (the matched track)
        #[arg(long)]
        matched: bool,
        /// the corpus root (its `truth/`): every selected function is a row,
        /// also one no tool has a result for, so the denominator is the corpus
        /// and not the union of the result files
        #[arg(long)]
        corpus: Option<PathBuf>,
        /// `freshbench duplicates` reports (all groups: --top 1000000), one per
        /// architecture: keep one function per clone group
        #[arg(long, num_args = 1..)]
        dedup: Vec<PathBuf>,
        #[arg(long, value_enum, default_value = "renamed")]
        dedup_level: freshbench::clones::Level,
        /// function-boundary rows (files or directories), shown with the main metrics
        #[arg(long, num_args = 1..)]
        boundaries: Vec<PathBuf>,
        /// timing (files or directories): what each tool cost
        #[arg(long, num_args = 1..)]
        speed: Vec<PathBuf>,
        /// tool registry (ci/tools.json); with --last-commits and --now, only
        /// maintained, registered, non-LLM tools are published
        #[arg(long)]
        tools: Option<PathBuf>,
        /// JSON {tool: date of the last upstream commit}
        #[arg(long)]
        last_commits: Option<PathBuf>,
        /// publication date (YYYY-MM-DD)
        #[arg(long)]
        now: Option<String>,
        #[arg(long, default_value_t = 365)]
        max_idle_days: i64,
    },
    /// DWARF type ground truth (return, parameter and local classes) of a function.
    Signature {
        binary: PathBuf,
        #[arg(long, value_parser = parse_u64)]
        address: u64,
    },
    /// Recompile a tool's outputs with each build's compiler and compare with the original code;
    /// writes JSON lines that `report`/`gaps` attach to the rows (name it `<dir>/rows/<tool>.recompile.jsonl`).
    Recompile {
        root: PathBuf,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        out: PathBuf,
        /// "image" = the build image's fbcc wrappers (default); or an LLVM bin directory (x86-64 only, for local checks)
        #[arg(long, default_value = "image")]
        toolchain: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Improvement targets of one tool against the others on the same rows.
    Gaps {
        root: PathBuf,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        json: Option<PathBuf>,
        #[arg(long, default_value_t = 5)]
        keep: usize,
    },
    /// Perfect rate and median normalized GED per tool and size band.
    Bands {
        root: PathBuf,
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Same ground truth + same output text with different scores (must be none).
    Determinism {
        root: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Build, verify or summarize a corpus manifest.
    Manifest {
        #[command(subcommand)]
        action: ManifestCmd,
    },
    /// Call-site agreement of one function (objdump of the UNSTRIPPED binary).
    Callsites {
        #[arg(long)]
        objdump_text: PathBuf,
        #[arg(long, value_parser = parse_u64)]
        address: u64,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        function: String,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        delta: i64,
    },
}

#[derive(Subcommand)]
enum ProvCmd {
    /// Write one job's provenance.json.
    Write {
        #[arg(long)]
        pins: PathBuf,
        #[arg(long)]
        tool: String,
        #[arg(long, default_value = "")]
        fission_sha: String,
        #[arg(long)]
        freshbench_commit: String,
        #[arg(long)]
        qualify_run: String,
        #[arg(long)]
        evaluate_run: String,
        #[arg(long)]
        arch: String,
        #[arg(long)]
        project: String,
        #[arg(long, default_value = "")]
        build: String,
        #[arg(long, default_value = "decompile")]
        track: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Page metadata from every provenance.json under DIR; fails on mixed versions.
    Meta {
        dir: PathBuf,
        #[arg(long)]
        corpus: String,
        #[arg(long)]
        note: Vec<String>,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum SweepCmd {
    /// Decompile every selected function under ROOT and score it. Resumes from
    /// the run's per-binary checkpoints.
    Run {
        root: PathBuf,
        /// the fission_cli to measure
        #[arg(long)]
        cli: PathBuf,
        /// the Fission checkout it was built from (commit, `utils/` resources)
        #[arg(long)]
        repo: PathBuf,
        /// the run's name: results go to <runs>/<label>
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "runs")]
        runs: PathBuf,
        #[arg(long)]
        resource_root: Option<PathBuf>,
        /// binaries at once; each fission_cli gets cores/jobs threads. 0 = half
        /// the cores (on 14 cores: 7 at once took 75 s where 4 took 131 s and
        /// 12 took 86 s, same outputs)
        #[arg(long, default_value_t = 0)]
        jobs: usize,
        /// only these arch-qualified builds (comma list)
        #[arg(long, value_delimiter = ',')]
        builds: Vec<String>,
        /// only binaries whose `<build>/<project>/<binary>` contains this
        #[arg(long)]
        only: Option<String>,
        #[arg(long, default_value_t = 1800)]
        binary_timeout_s: u64,
        #[arg(long, default_value_t = 300)]
        function_timeout_s: u64,
        #[arg(long, default_value_t = freshbench::ged::DEFAULT_BUDGET)]
        budget: u64,
        /// decompile again what is already checkpointed
        #[arg(long)]
        force: bool,
    },
    /// Scores, failures and timing of one run (markdown; `--json` for the numbers).
    Report {
        run: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Compare two runs: output hashes first, scores only on rows whose output changed.
    Diff {
        a: PathBuf,
        b: PathBuf,
        #[arg(long)]
        json: bool,
        /// rows listed in each direction
        #[arg(long, default_value_t = 15)]
        keep: usize,
    },
}

#[derive(Subcommand)]
enum ManifestCmd {
    Build {
        root: PathBuf,
        out: PathBuf,
        /// JSON {"builds": {"project|build": {...}}, "projects": {"project": {...}}}
        #[arg(long)]
        provenance: Option<PathBuf>,
    },
    Verify {
        root: PathBuf,
        manifest: PathBuf,
    },
    Summary {
        manifest: PathBuf,
    },
}

fn parse_u64(s: &str) -> Result<u64, String> {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).map_err(|e| e.to_string()),
        None => s
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string()),
    }
}

fn main() -> ExitCode {
    #[cfg(feature = "sentry")]
    let _telemetry = telemetry::init();
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn read(p: &PathBuf) -> Result<String> {
    let b = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, v)?;
    writeln!(out)?;
    Ok(())
}

fn run() -> Result<ExitCode> {
    match Cli::parse().cmd {
        Cmd::Score {
            source,
            output,
            function,
        } => {
            let (s, o) = (read(&source)?, read(&output)?);
            match score::score(&s, &o, &function) {
                Ok(r) => print_json(&r)?,
                Err(u) => {
                    print_json(&serde_json::json!({ "unscored": u }))?;
                    return Ok(ExitCode::from(1));
                }
            }
        }
        Cmd::Rescore {
            root,
            tool,
            out,
            limit_tus,
            budget,
            cache,
            builds,
        } => {
            let sc = match &cache {
                Some(p) => freshbench::dataset::ScoreCache::load(p)?,
                None => freshbench::dataset::ScoreCache::default(),
            };
            let t0 = std::time::Instant::now();
            let rows = freshbench::dataset::rescore(&root, &tool, limit_tus, budget, &sc, &builds)?;
            let (h, m) = (
                sc.hits.load(std::sync::atomic::Ordering::Relaxed),
                sc.misses.load(std::sync::atomic::Ordering::Relaxed),
            );
            eprintln!(
                "score cache: {h} hits, {m} scored; {:.1} s",
                t0.elapsed().as_secs_f64()
            );
            if let Some(p) = &cache {
                let n = sc.save(p)?;
                eprintln!("score cache: {n} new entries appended to {}", p.display());
            }
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut w = std::io::BufWriter::new(std::fs::File::create(&out)?);
            let mut counts: BTreeMap<String, u64> = BTreeMap::new();
            for r in &rows {
                serde_json::to_writer(&mut w, r)?;
                w.write_all(b"\n")?;
                *counts
                    .entry(headroom::outcome_label(r).to_string())
                    .or_default() += 1;
                if r.ged.as_ref().is_some_and(|g| !g.exact) {
                    *counts.entry("ged_upper_bound_only".into()).or_default() += 1;
                }
            }
            eprintln!("{tool}: {} rows {counts:?}", rows.len());
        }
        Cmd::Eligible {
            build_dir,
            project,
            submodules,
            exclude,
            json,
        } => {
            let r = freshbench::prepare::analyse_build_dir(
                &build_dir,
                &project,
                &submodules,
                exclude.as_deref(),
            )?;
            if let Some(p) = json {
                std::fs::write(&p, serde_json::to_vec_pretty(&r)?)?;
            }
            eprintln!("{}", serde_json::to_string(&r.stats)?);
            println!("eligible {}", r.eligible.len());
        }
        Cmd::Prepare {
            project,
            builds,
            out,
            submodules,
            exclude,
            select,
            strip_tool,
            only,
            count_only,
        } => {
            std::fs::create_dir_all(&out)?;
            let s = freshbench::prepare::prepare(
                &project,
                &builds,
                &out,
                &submodules,
                exclude.as_deref(),
                select,
                &strip_tool,
                &only,
                count_only,
            )?;
            std::fs::write(
                out.join("prepare_summary.json"),
                serde_json::to_vec_pretty(&s)?,
            )?;
            for (k, v) in &s.builds {
                println!(
                    "{k}: eligible {} selected {}",
                    v["eligible"],
                    v.get("selected").cloned().unwrap_or_default()
                );
            }
            println!(
                "{}",
                if s.qualifies {
                    "QUALIFIES"
                } else {
                    "DOES NOT QUALIFY"
                }
            );
        }
        Cmd::Duplicates {
            root,
            arch,
            top,
            against,
        } => print_json(&freshbench::clones::audit(
            &root,
            arch.as_deref(),
            top,
            against.as_deref(),
        )?)?,
        Cmd::Fingerprints { dir, out } => {
            use rayon::prelude::*;
            let mut files = Vec::new();
            rows::walk_sorted(&dir, &mut |p| {
                if p.extension().is_some_and(|e| e == "c" || e == "i") {
                    files.push(p.to_path_buf());
                }
            })?;
            let sets: Vec<Vec<String>> = files
                .par_iter()
                .map(|p| {
                    let t = std::fs::read(p)
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default();
                    let Ok(tree) = csyntax::parse(&t) else {
                        return vec![];
                    };
                    csyntax::definitions(&tree, &t)
                        .into_values()
                        .flatten()
                        .map(|n| freshbench::prepare::fingerprint(n, &t))
                        .collect()
                })
                .collect();
            let all: std::collections::BTreeSet<String> = sets.into_iter().flatten().collect();
            std::fs::write(&out, serde_json::to_vec(&all)?)?;
            eprintln!(
                "{} files, {} distinct function fingerprints",
                files.len(),
                all.len()
            );
        }
        Cmd::Decompile {
            root,
            tool,
            fission_cli,
            resource_root,
            adapter,
            binary_timeout_s,
            function_timeout_s,
            builds,
            jobs,
        } => {
            use freshbench::adapters::{self, Tool};
            use std::time::Duration;
            adapters::refuse_instrumented_run("decompile")?;
            let t = if tool == "fission" {
                Tool::Fission {
                    cli: fission_cli.context("--fission-cli is required for fission")?,
                    resource_root,
                    function_timeout_ms: function_timeout_s * 1000,
                    rayon_threads: None,
                }
            } else {
                anyhow::ensure!(
                    !adapter.is_empty(),
                    "--adapter <command...> is required for {tool}"
                );
                Tool::Script {
                    name: tool.clone(),
                    command: adapter,
                }
            };
            use rayon::prelude::*;
            let units: Vec<_> = freshbench::corpus::units(&root)?
                .into_iter()
                // --builds takes arch-qualified build names (`gcc-O0`, `aarch64-gcc-O0`)
                .filter(|u| builds.is_empty() || builds.contains(&u.build))
                .collect();
            // one job per binary, largest first so a long one starts early;
            // results are collected in a fixed order, so output does not depend on --jobs
            let work: Vec<(usize, &String)> = units
                .iter()
                .enumerate()
                .flat_map(|(i, u)| u.targets.binaries.keys().map(move |b| (i, b)))
                .collect();
            let size = |&(i, b): &(usize, &String)| {
                std::fs::metadata(root.join(&units[i].stripped).join(b)).map_or(0, |m| m.len())
            };
            let mut order: Vec<usize> = (0..work.len()).collect();
            order.sort_by_key(|&k| std::cmp::Reverse(size(&work[k])));
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(jobs.max(1))
                .build()?;
            type Done = (Vec<adapters::Record>, adapters::CallStats);
            let done: Vec<(usize, Result<Done>)> = pool.install(|| {
                order
                    .par_iter()
                    .map(|&k| {
                        let (i, bin) = work[k];
                        let b = &units[i].targets.binaries[bin];
                        // Thumb functions are requested as address | 1 (the ELF for ARM convention)
                        let funcs: BTreeMap<u64, String> = b
                            .functions
                            .iter()
                            .map(|f| (f.address | u64::from(f.thumb), f.name.clone()))
                            .collect();
                        let r = adapters::decompile_binary_stats(
                            &t,
                            &root.join(&units[i].stripped).join(bin),
                            bin,
                            &funcs,
                            Duration::from_secs(binary_timeout_s),
                            Duration::from_secs(function_timeout_s),
                        );
                        eprintln!("{} {bin}: done", units[i].build);
                        (k, r)
                    })
                    .collect()
            });
            let mut by_k: Vec<Option<Done>> = vec![None; work.len()];
            for (k, r) in done {
                by_k[k] = Some(r?);
            }
            let (mut ok, mut failed) = (0u64, 0u64);
            for (i, u) in units.iter().enumerate() {
                let mut recs = Vec::new();
                let mut timing = String::new();
                let pb = &u.targets;
                for (k, (wi, bin)) in work.iter().enumerate() {
                    if *wi == i {
                        let (r, stats) = by_k[k].take().unwrap_or_default();
                        let line = freshbench::speed::Timing {
                            tool: t.name().to_string(),
                            track: "decompile".into(),
                            project: pb.project.clone(),
                            arch: pb.arch.clone(),
                            compiler: pb.compiler.clone(),
                            config: pb.config.clone(),
                            binary: (*bin).clone(),
                            functions: r.len() as u64,
                            calls: stats.calls,
                            failed_calls: stats.failed_calls,
                            jobs: jobs.max(1) as u32,
                            usage: stats.usage,
                        };
                        timing.push_str(&serde_json::to_string(&line)?);
                        timing.push('\n');
                        recs.extend(r);
                    }
                }
                ok += recs.iter().filter(|x| x.code.is_some()).count() as u64;
                failed += recs.iter().filter(|x| x.code.is_none()).count() as u64;
                // results are keyed by the arch-qualified build, so rows of a
                // non-x86-64 build never read back as x86-64
                let out = root
                    .join("results")
                    .join(t.name())
                    .join(&u.build)
                    .join(&pb.project)
                    .join("records.jsonl.gz");
                adapters::write_records(&out, &recs)?;
                // what each binary cost: wall, CPU and peak memory of the tool
                std::fs::write(out.with_file_name("timing.jsonl"), timing)?;
                eprintln!("{}/{}: {} functions", u.build, pb.project, recs.len());
            }
            eprintln!("{}: {ok} decompiled, {failed} without output", t.name());
        }
        Cmd::Boundaries {
            root,
            tool,
            fission_cli,
            resource_root,
            adapter,
            out,
            binary_timeout_s,
            builds,
            jobs,
            variant,
        } => {
            use freshbench::adapters::{self, Tool};
            use freshbench::boundaries;
            adapters::refuse_instrumented_run("boundaries")?;
            anyhow::ensure!(
                variant == "as-built" || variant == "no-unwind",
                "--variant is as-built or no-unwind"
            );
            let scratch = tempfile::tempdir()?;
            use rayon::prelude::*;
            let t = if tool == "fission" {
                Tool::Fission {
                    cli: fission_cli.context("--fission-cli is required for fission")?,
                    resource_root,
                    function_timeout_ms: 0,
                    rayon_threads: None,
                }
            } else {
                anyhow::ensure!(
                    !adapter.is_empty(),
                    "--adapter <command...> is required for {tool}"
                );
                Tool::Script {
                    name: tool.clone(),
                    command: adapter,
                }
            };
            let units: Vec<_> = freshbench::corpus::units(&root)?
                .into_iter()
                .filter(|u| builds.is_empty() || builds.contains(&u.build))
                .collect();
            let work: Vec<(usize, &String)> = units
                .iter()
                .enumerate()
                .flat_map(|(i, u)| u.targets.binaries.keys().map(move |b| (i, b)))
                .collect();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(jobs.max(1))
                .build()?;
            let rows: Vec<Result<boundaries::Row>> = pool.install(|| {
                work.par_iter()
                    .map(|&(i, bin)| {
                        let u = &units[i];
                        let pb = &u.targets;
                        let full = std::fs::read(root.join(&u.binaries).join(bin))?;
                        let truth = boundaries::truth(&full)?;
                        let plt = boundaries::plt_ranges(&full)?;
                        let arm = pb.arch == "armv7";
                        let mut input = root.join(&u.stripped).join(bin);
                        if variant == "no-unwind" {
                            let (bytes, _) = boundaries::without_unwind(&std::fs::read(&input)?)?;
                            let p = scratch.path().join(format!("{i}-{bin}"));
                            std::fs::write(&p, bytes)?;
                            input = p;
                        }
                        let listed = adapters::list_functions_measured(
                            &t,
                            &input,
                            std::time::Duration::from_secs(binary_timeout_s),
                        );
                        if variant == "no-unwind" {
                            let _ = std::fs::remove_file(&input);
                        }
                        let (counts, error, usage) = match listed {
                            Ok((found, usage)) => (
                                boundaries::score(&truth, &found, arm, &plt),
                                None,
                                Some(usage),
                            ),
                            Err(e) => (
                                boundaries::score(&truth, &[], arm, &plt),
                                Some(format!("{e:#}").chars().take(300).collect()),
                                None,
                            ),
                        };
                        eprintln!(
                            "{} {bin}: {} of {} starts",
                            u.build, counts.start_match, counts.truth
                        );
                        Ok(boundaries::Row {
                            tool: t.name().to_string(),
                            variant: variant.clone(),
                            project: pb.project.clone(),
                            arch: pb.arch.clone(),
                            compiler: pb.compiler.clone(),
                            config: pb.config.clone(),
                            binary: bin.clone(),
                            stripped_sha256: pb.binaries[bin].stripped_sha256.clone(),
                            counts,
                            error,
                            usage,
                            jobs: jobs.max(1) as u32,
                        })
                    })
                    .collect()
            });
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut f = std::io::BufWriter::new(std::fs::File::create(&out)?);
            for r in rows {
                serde_json::to_writer(&mut f, &r?)?;
                std::io::Write::write_all(&mut f, b"\n")?;
            }
        }
        Cmd::BoundariesSummary { rows: paths } => {
            let rows = freshbench::boundaries::load(&paths)?;
            print_json(&freshbench::boundaries::summarize(&rows))?
        }
        Cmd::Provenance { action } => match action {
            ProvCmd::Write {
                pins,
                tool,
                fission_sha,
                freshbench_commit,
                qualify_run,
                evaluate_run,
                arch,
                project,
                build,
                track,
                out,
            } => {
                use freshbench::provenance as pv;
                let pins = pv::read_pins(&read(&pins)?);
                let p = pv::Provenance {
                    tool_version: pv::tool_version(&tool, &pins, &fission_sha)?,
                    tool,
                    freshbench_commit,
                    qualify_run,
                    evaluate_run,
                    arch,
                    project,
                    build: if build.is_empty() {
                        "all".into()
                    } else {
                        build
                    },
                    track,
                };
                if let Some(d) = out.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&out, serde_json::to_vec_pretty(&p)?)?;
            }
            ProvCmd::Meta {
                dir,
                corpus,
                note,
                out,
            } => {
                use freshbench::provenance as pv;
                let m = pv::meta(&pv::load(&dir)?, &corpus, &note)?;
                std::fs::write(&out, serde_json::to_vec_pretty(&m)?)?;
            }
        },
        Cmd::Quickcount { build_dir } => {
            println!("{}", freshbench::prepare::quick_count(&build_dir)?)
        }
        Cmd::Status { file, pairs } => freshbench::provenance::set_status(&file, &pairs)?,
        Cmd::Speed { paths, json } => {
            let s = freshbench::speed::summarize(&freshbench::speed::load(&paths)?);
            if json {
                print_json(&s)?
            } else {
                print!("{}", freshbench::speed::render(&s))
            }
        }
        Cmd::Sweep { action } => match action {
            SweepCmd::Run {
                root,
                cli,
                repo,
                label,
                runs,
                resource_root,
                jobs,
                builds,
                only,
                binary_timeout_s,
                function_timeout_s,
                budget,
                force,
            } => {
                freshbench::adapters::refuse_instrumented_run("sweep run")?;
                let run = freshbench::sweep::run(&freshbench::sweep::RunArgs {
                    root,
                    cli,
                    repo,
                    label,
                    runs,
                    resource_root,
                    jobs,
                    builds,
                    only,
                    binary_timeout_s,
                    function_timeout_s,
                    budget,
                    force,
                })?;
                print!(
                    "{}",
                    freshbench::sweep::render_report(&freshbench::sweep::report(&run)?)
                );
            }
            SweepCmd::Report { run, json } => {
                let r = freshbench::sweep::report(&run)?;
                if json {
                    print_json(&r)?
                } else {
                    print!("{}", freshbench::sweep::render_report(&r))
                }
            }
            SweepCmd::Diff { a, b, json, keep } => {
                let d = freshbench::sweep::diff(&a, &b, keep)?;
                if json {
                    print_json(&d)?
                } else {
                    print!("{}", freshbench::sweep::render_diff(&d))
                }
            }
        },
        Cmd::Assemble {
            prepared,
            out,
            license,
        } => {
            let n = freshbench::corpus::assemble(&prepared, license.as_deref(), &out)?;
            eprintln!(
                "{}: {n} files written to {}",
                prepared.display(),
                out.display()
            );
        }
        Cmd::Report {
            root,
            out,
            meta,
            resamples,
            keep,
            matched,
            corpus,
            dedup,
            dedup_level,
            boundaries,
            speed,
            tools,
            last_commits,
            now,
            max_idle_days,
        } => {
            let mut meta: freshbench::report::Meta = match meta {
                Some(p) => serde_json::from_str(&read(&p)?)?,
                None => Default::default(),
            };
            let mut table = rows::load_table(&root)?;
            if let Some(c) = &corpus {
                let before = table.len();
                let targets = freshbench::dataset::targets(c)?;
                let known: std::collections::BTreeSet<_> =
                    targets.iter().map(|t| t.key.clone()).collect();
                let stray = table.keys().filter(|k| !known.contains(*k)).count();
                anyhow::ensure!(
                    stray == 0,
                    "{stray} result rows are not functions of the corpus at {}",
                    c.display()
                );
                for t in targets {
                    table.entry(t.key).or_default();
                }
                meta.notes.push(format!(
                    "denominator: the corpus's {} selected functions ({} had no result from any tool)",
                    table.len(),
                    table.len() - before
                ));
            }
            let all = table.len();
            if !dedup.is_empty() {
                let reps: Vec<freshbench::clones::Report> = dedup
                    .iter()
                    .map(|d| Ok(serde_json::from_str(&read(d)?)?))
                    .collect::<Result<_>>()?;
                table = freshbench::report::dedup(&table, &reps, dedup_level)?;
                meta.notes.push(format!(
                    "one function per {dedup_level:?} clone group: {} of {all} rows",
                    table.len()
                ));
            }
            if matched {
                let before = table.len();
                table = freshbench::report::matched(&table);
                meta.notes.push(format!(
                    "matched track: functions present in every build, {} of {before} rows",
                    table.len()
                ));
            }
            let mut brows = freshbench::boundaries::load(&boundaries)?;
            let mut trows = freshbench::speed::load(&speed)?;
            if let Some(reg) = &tools {
                let registry: std::collections::BTreeMap<String, freshbench::policy::ToolInfo> =
                    serde_json::from_str(&read(reg)?)?;
                let last: std::collections::BTreeMap<String, String> = match &last_commits {
                    Some(p) => serde_json::from_str(&read(p)?)?,
                    None => Default::default(),
                };
                let now = now.context("--now is required with --tools")?;
                let present: std::collections::BTreeSet<String> = table
                    .values()
                    .flat_map(|m| m.keys().cloned())
                    .chain(brows.iter().map(|r| r.tool.clone()))
                    .chain(trows.iter().map(|r| r.tool.clone()))
                    .collect();
                let (keep_tools, notes) = freshbench::policy::publishable(
                    &present,
                    &registry,
                    &last,
                    &now,
                    max_idle_days,
                )?;
                for m in table.values_mut() {
                    m.retain(|t, _| keep_tools.contains(t));
                }
                table.retain(|_, m| !m.is_empty() || corpus.is_some());
                brows.retain(|r| keep_tools.contains(&r.tool));
                trows.retain(|r| keep_tools.contains(&r.tool));
                meta.notes.extend(notes);
            }
            let extras = freshbench::report::Extras {
                boundaries: (!brows.is_empty()).then(|| freshbench::boundaries::summarize(&brows)),
                speed: (!trows.is_empty()).then(|| freshbench::speed::summarize(&trows)),
            };
            std::fs::create_dir_all(&out)?;
            let files = freshbench::report::site_with(&table, &meta, resamples, keep, &extras);
            for (name, body) in &files {
                std::fs::write(out.join(name), body)?;
            }
            eprintln!(
                "{} files, {} rows, written to {}",
                files.len(),
                table.len(),
                out.display()
            );
        }
        Cmd::Signature { binary, address } => {
            let sigs = freshbench::dwarf::signatures(&std::fs::read(&binary)?)?;
            print_json(&sigs.get(&address))?;
        }
        Cmd::Recompile {
            root,
            tool,
            out,
            toolchain,
            limit,
        } => {
            use freshbench::recompile::{image_toolchain, Toolchain};
            let llvm = (toolchain != "image").then(|| PathBuf::from(&toolchain));
            let tc = move |arch: &str, compiler: &str, flags: &str| -> Toolchain {
                match &llvm {
                    None => image_toolchain(arch, compiler, flags),
                    Some(dir) => Toolchain {
                        cc: vec![
                            dir.join("clang").display().to_string(),
                            "--target=x86_64-linux-gnu".into(),
                        ],
                        env: vec![],
                        flags: flags.split_whitespace().map(str::to_string).collect(),
                        objdump: dir.join("llvm-objdump"),
                        arch: "x86_64".into(),
                    },
                }
            };
            let lines = freshbench::dataset::recompile_all(&root, &tool, &tc, limit)?;
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut w = std::io::BufWriter::new(std::fs::File::create(&out)?);
            let (mut compiled, mut exact) = (0, 0);
            for l in &lines {
                compiled += u64::from(l.recompile.compiles);
                exact += u64::from(l.recompile.exact);
                serde_json::to_writer(&mut w, l)?;
                w.write_all(b"\n")?;
            }
            eprintln!(
                "{tool}: {} functions, {compiled} compile, {exact} exact",
                lines.len()
            );
        }
        Cmd::Cfg { file, function } => {
            let t = read(&file)?;
            let tree = csyntax::parse(&t)?;
            let def = csyntax::output_function(&tree, &t, &function)
                .context("no single definition of the function")?;
            print_json(&cfg::build(def, &t))?;
        }
        Cmd::Gaps {
            root,
            tool,
            json,
            keep,
        } => {
            let table = rows::load_table(&root)?;
            let h = headroom::compute(&table, &tool, None, keep);
            if !h.tools.contains(&tool) {
                eprintln!("tool {tool:?} not in {:?}", h.tools);
                return Ok(ExitCode::from(2));
            }
            if let Some(p) = json {
                std::fs::write(&p, serde_json::to_vec_pretty(&h)?)?;
            }
            println!("{}", render_gaps(&h));
        }
        Cmd::Bands { root, json } => {
            #[derive(Default, serde::Serialize)]
            struct Cell {
                rows: u64,
                perfect: u64,
                #[serde(skip)]
                norm: Vec<f64>,
                median_normalized_ged: Option<f64>,
            }
            let mut per: BTreeMap<String, BTreeMap<&'static str, Cell>> = BTreeMap::new();
            rows::for_each_row(&root, |r| {
                let g = r.ged.as_ref();
                let b = bands::band(g.and_then(|g| g.source_nodes));
                let c = per.entry(r.tool.clone()).or_default().entry(b).or_default();
                c.rows += 1;
                if r.scored_ged() == Some(0.0) {
                    c.perfect += 1;
                }
                if let Some(n) = bands::normalized_ged(
                    r.scored_ged(),
                    g.and_then(|g| g.source_nodes),
                    g.and_then(|g| g.source_edges),
                ) {
                    c.norm.push(n);
                }
            })?;
            println!("| tool | band | rows | perfect | perfect % | median normalized GED |\n|---|---|---:|---:|---:|---:|");
            for (tool, m) in per.iter_mut() {
                for b in bands::labels().chain([bands::UNKNOWN]) {
                    let Some(c) = m.get_mut(b) else { continue };
                    c.norm.sort_by(f64::total_cmp);
                    c.median_normalized_ged =
                        (!c.norm.is_empty()).then(|| c.norm[c.norm.len() / 2]);
                    let pct = 100.0 * c.perfect as f64 / c.rows.max(1) as f64;
                    let med = c
                        .median_normalized_ged
                        .map(|m| format!("{m:.3}"))
                        .unwrap_or_default();
                    println!(
                        "| {tool} | {b} | {} | {} | {pct:.1}% | {med} |",
                        c.rows, c.perfect
                    );
                }
            }
            if let Some(p) = json {
                std::fs::write(&p, serde_json::to_vec_pretty(&per)?)?;
            }
        }
        Cmd::Determinism { root, limit } => {
            let mut seen: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
            rows::for_each_row(&root, |r| {
                if let (Some(g), Some(sha)) = (r.scored_ged(), r.code_sha256.clone()) {
                    let v = seen.entry((r.key.to_string(), sha)).or_default();
                    if !v.contains(&g) {
                        v.push(g);
                    }
                }
            })?;
            let bad: Vec<_> = seen.iter().filter(|(_, v)| v.len() > 1).collect();
            for ((rid, sha), v) in bad.iter().take(limit) {
                println!("{rid}@{}\t{v:?}", &sha[..12.min(sha.len())]);
            }
            println!(
                "{} (ground truth, output) pairs with more than one GED",
                bad.len()
            );
            if !bad.is_empty() {
                return Ok(ExitCode::from(1));
            }
        }
        Cmd::Manifest { action } => match action {
            ManifestCmd::Build {
                root,
                out,
                provenance,
            } => {
                let mut prov = manifest::Provenance::default();
                if let Some(p) = provenance {
                    let v: serde_json::Value = serde_json::from_str(&read(&p)?)?;
                    if let Some(b) = v.get("builds").and_then(|b| b.as_object()) {
                        for (k, x) in b {
                            let (proj, build) = k
                                .split_once('|')
                                .context("builds key must be project|build")?;
                            prov.builds.insert(
                                (proj.into(), build.into()),
                                serde_json::from_value(x.clone())?,
                            );
                        }
                    }
                    if let Some(p) = v.get("projects").and_then(|b| b.as_object()) {
                        for (k, x) in p {
                            prov.projects
                                .insert(k.clone(), serde_json::from_value(x.clone())?);
                        }
                    }
                }
                // binaries of an assembled corpus carry their build's exact
                // compiler version line and flags
                if root.join("truth").is_dir() {
                    for u in freshbench::corpus::units(&root)? {
                        let t = &u.targets;
                        let e = prov
                            .builds
                            .entry((t.project.clone(), u.build.clone()))
                            .or_default();
                        e.entry("compiler_version".into())
                            .or_insert_with(|| t.compiler_version.clone().into());
                        e.entry("flags".into())
                            .or_insert_with(|| t.flags.clone().into());
                    }
                }
                let entries = manifest::build(&root, &prov)?;
                manifest::write(&entries, &out)?;
                print_json(&manifest::summarize(&entries))?;
            }
            ManifestCmd::Verify { root, manifest: m } => {
                let problems = manifest::verify(&root, &manifest::read(&m)?)?;
                for p in problems.iter().take(200) {
                    println!("{p}");
                }
                println!("{} problems", problems.len());
                if !problems.is_empty() {
                    return Ok(ExitCode::from(1));
                }
            }
            ManifestCmd::Summary { manifest: m } => {
                print_json(&manifest::summarize(&manifest::read(&m)?))?
            }
        },
        Cmd::Callsites {
            objdump_text,
            address,
            output,
            function,
            delta,
        } => {
            let funcs = callsites::parse_objdump(&read(&objdump_text)?);
            let a2n = callsites::symbol_table(&funcs);
            let f = funcs
                .get(&address)
                .context("no function at that address in the objdump")?;
            let code = read(&output)?;
            let tree = csyntax::parse(&code)?;
            let def = csyntax::output_function(&tree, &code, &function)
                .context("no function in the output")?;
            let feats = csyntax::features(def, &code);
            print_json(&callsites::compare(&f.calls, &feats.calls, &a2n, delta))?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn render_gaps(h: &headroom::Headroom) -> String {
    let o = &h.overall;
    let others: Vec<_> = h.tools.iter().filter(|t| **t != h.tool).cloned().collect();
    let mut s = format!("# {}: headroom against {}\n\n", h.tool, others.join(", "));
    s += &format!(
        "rows {}; perfect {}; reachable (another tool perfect, {} not) {}; no score where another tool scored {}\n",
        o.rows, o.perfect, h.tool, o.reachable, o.uncovered
    );
    s += &format!(
        "GED on {} rows both scored: {} {:.0}, per-row best of all tools {:.0}; gap to the best other tool {:.0}\n\n",
        h.common_rows, h.tool, h.tool_ged_on_common, h.virtual_best_ged_on_common, o.gap
    );
    s += "| band | rows | perfect | reachable | gap | no score |\n|---|---:|---:|---:|---:|---:|\n";
    for b in bands::labels().chain([bands::UNKNOWN]) {
        if let Some(g) = h.by_band.get(b) {
            s += &format!(
                "| {b} | {} | {} | {} | {:.0} | {} |\n",
                g.rows, g.perfect, g.reachable, g.gap, g.uncovered
            );
        }
    }
    s += "\n| outcome | rows | reachable | gap |\n|---|---:|---:|---:|\n";
    for (k, g) in &h.by_outcome {
        s += &format!("| {k} | {} | {} | {:.0} |\n", g.rows, g.reachable, g.gap);
    }
    if !h.by_tag.is_empty() {
        s += "\nShape tags (a row counts under each of its tags; totals overlap):\n\n| tag | rows | reachable | gap | worst rows |\n|---|---:|---:|---:|---|\n";
        let mut tags: Vec<_> = h.by_tag.iter().collect();
        tags.sort_by(|a, b| b.1.gap.total_cmp(&a.1.gap).then(a.0.cmp(b.0)));
        for (t, g) in tags {
            let ex: Vec<_> = g
                .examples
                .iter()
                .take(3)
                .map(|e| format!("{} ({:.0})", e.row, e.gap))
                .collect();
            s += &format!(
                "| {t} | {} | {} | {:.0} | {} |\n",
                g.rows,
                g.reachable,
                g.gap,
                ex.join(", ")
            );
        }
    }
    s += "\nSimilarity gaps (GED), not correctness defects.";
    s
}

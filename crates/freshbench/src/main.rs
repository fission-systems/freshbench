use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use freshbench::{bands, callsites, cfg, csyntax, headroom, manifest, rows, score};

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
    /// Score a tool's decompiled code in a dataset tree with FreshBench's metrics (v2 rows, JSON lines).
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
        } => {
            let rows = freshbench::dataset::rescore(&root, &tool, limit_tus, budget)?;
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

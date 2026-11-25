//! How fast each tool is: wall time, CPU time and peak memory per binary.
//!
//! `decompile` writes one line per binary to
//! `results/<tool>/<build>/<project>/timing.jsonl`, and the boundary track
//! carries the same figures in its rows. Wall time depends on how many
//! binaries ran at once and on the machine; CPU time (user + system of the
//! tool's process tree) is the figure to compare across tools. Peak memory is
//! the largest process of the tree.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::adapters::Usage;

/// One binary through one tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub tool: String,
    /// `decompile`, or `boundaries as-built` / `boundaries no-unwind`
    pub track: String,
    pub project: String,
    pub arch: String,
    pub compiler: String,
    pub config: String,
    pub binary: String,
    /// functions asked for (decompile) or found (boundaries)
    pub functions: u64,
    pub calls: u32,
    pub failed_calls: u32,
    /// binaries the harness ran at once when this was measured
    pub jobs: u32,
    #[serde(flatten)]
    pub usage: Usage,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Speed {
    pub binaries: u64,
    pub functions: u64,
    pub wall_s: f64,
    pub cpu_s: f64,
    /// CPU milliseconds per function
    pub cpu_ms_per_function: f64,
    pub wall_ms_per_function: f64,
    pub median_cpu_s_per_binary: f64,
    pub p90_cpu_s_per_binary: f64,
    pub max_cpu_s_per_binary: f64,
    pub median_rss_mb: f64,
    pub max_rss_mb: f64,
    pub failed_calls: u64,
    #[serde(skip)]
    cpu: Vec<f64>,
    #[serde(skip)]
    rss: Vec<f64>,
}

impl Speed {
    fn add(&mut self, t: &Timing) {
        self.binaries += 1;
        self.functions += t.functions;
        self.wall_s += t.usage.wall_s;
        self.cpu_s += t.usage.cpu_s;
        self.failed_calls += u64::from(t.failed_calls);
        self.cpu.push(t.usage.cpu_s);
        self.rss.push(t.usage.max_rss_mb);
    }

    fn finish(&mut self) {
        let q = |v: &mut Vec<f64>, p: f64| {
            v.sort_by(f64::total_cmp);
            if v.is_empty() {
                0.0
            } else {
                v[((v.len() - 1) as f64 * p).round() as usize]
            }
        };
        self.median_cpu_s_per_binary = q(&mut self.cpu, 0.5);
        self.p90_cpu_s_per_binary = q(&mut self.cpu, 0.9);
        self.max_cpu_s_per_binary = q(&mut self.cpu, 1.0);
        self.median_rss_mb = q(&mut self.rss, 0.5);
        self.max_rss_mb = q(&mut self.rss, 1.0);
        let f = self.functions.max(1) as f64;
        self.cpu_ms_per_function = 1000.0 * self.cpu_s / f;
        self.wall_ms_per_function = 1000.0 * self.wall_s / f;
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    /// track -> tool
    pub overall: BTreeMap<String, BTreeMap<String, Speed>>,
    /// track -> arch -> tool
    pub by_arch: BTreeMap<String, BTreeMap<String, BTreeMap<String, Speed>>>,
}

pub fn summarize(rows: &[Timing]) -> Summary {
    let mut s = Summary::default();
    for t in rows {
        s.overall
            .entry(t.track.clone())
            .or_default()
            .entry(t.tool.clone())
            .or_default()
            .add(t);
        s.by_arch
            .entry(t.track.clone())
            .or_default()
            .entry(t.arch.clone())
            .or_default()
            .entry(t.tool.clone())
            .or_default()
            .add(t);
    }
    for m in s.overall.values_mut() {
        m.values_mut().for_each(Speed::finish);
    }
    for a in s.by_arch.values_mut() {
        for m in a.values_mut() {
            m.values_mut().for_each(Speed::finish);
        }
    }
    s
}

/// Timing lines from `timing.jsonl` files and boundary rows under the paths.
pub fn load(paths: &[impl AsRef<Path>]) -> Result<Vec<Timing>> {
    let mut files = Vec::new();
    for p in paths {
        let p = p.as_ref();
        if p.is_dir() {
            crate::rows::walk_sorted(p, &mut |f| {
                if f.extension().is_some_and(|e| e == "jsonl") {
                    files.push(f.to_path_buf())
                }
            })?;
        } else {
            files.push(p.to_path_buf());
        }
    }
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            // a timing line, or a boundary row that carries its usage; other
            // JSON lines (score rows) are not timings
            if let Ok(t) = serde_json::from_str::<Timing>(line) {
                out.push(t);
            } else if let Ok(b) = serde_json::from_str::<crate::boundaries::Row>(line) {
                if let Some(usage) = b.usage {
                    out.push(Timing {
                        tool: b.tool,
                        track: format!("boundaries {}", b.variant),
                        project: b.project,
                        arch: b.arch,
                        compiler: b.compiler,
                        config: b.config,
                        binary: b.binary,
                        functions: b.counts.found,
                        calls: 1,
                        failed_calls: u32::from(b.error.is_some()),
                        jobs: b.jobs,
                        usage,
                    });
                }
            }
        }
    }
    Ok(out)
}

pub fn render(s: &Summary) -> String {
    let mut out = String::new();
    for (track, tools) in &s.overall {
        out += &format!(
            "## {track}\n\n| tool | binaries | functions | CPU ms / function | CPU s (total) | wall s (total) | median CPU s / binary | p90 | max | median RSS MB | max RSS MB | failed calls |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n"
        );
        let mut order: Vec<_> = tools.iter().collect();
        order.sort_by(|a, b| {
            a.1.cpu_ms_per_function
                .total_cmp(&b.1.cpu_ms_per_function)
                .then(a.0.cmp(b.0))
        });
        for (tool, v) in order {
            out += &format!(
                "| {tool} | {} | {} | {:.1} | {:.0} | {:.0} | {:.1} | {:.1} | {:.1} | {:.0} | {:.0} | {} |\n",
                v.binaries,
                v.functions,
                v.cpu_ms_per_function,
                v.cpu_s,
                v.wall_s,
                v.median_cpu_s_per_binary,
                v.p90_cpu_s_per_binary,
                v.max_cpu_s_per_binary,
                v.median_rss_mb,
                v.max_rss_mb,
                v.failed_calls
            );
        }
        out.push('\n');
    }
    out += "CPU time is user + system of the tool's whole process tree and is the figure to compare; wall time depends on how many binaries ran at once.\n";
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(tool: &str, arch: &str, functions: u64, cpu: f64, rss: f64) -> Timing {
        Timing {
            tool: tool.into(),
            track: "decompile".into(),
            project: "p".into(),
            arch: arch.into(),
            compiler: "gcc".into(),
            config: "O2".into(),
            binary: "b".into(),
            functions,
            calls: 1,
            failed_calls: 0,
            jobs: 2,
            usage: Usage {
                wall_s: cpu / 2.0,
                cpu_s: cpu,
                max_rss_mb: rss,
            },
        }
    }

    #[test]
    fn per_function_cost_and_quantiles() {
        let rows = [
            t("a", "x86_64", 100, 10.0, 200.0),
            t("a", "x86_64", 300, 30.0, 900.0),
            t("a", "armv7", 100, 20.0, 300.0),
            t("b", "x86_64", 100, 1.0, 50.0),
        ];
        let s = summarize(&rows);
        let a = &s.overall["decompile"]["a"];
        assert_eq!((a.binaries, a.functions), (3, 500));
        assert_eq!(a.cpu_s, 60.0);
        assert_eq!(a.cpu_ms_per_function, 120.0);
        assert_eq!(a.wall_ms_per_function, 60.0);
        assert_eq!(a.median_cpu_s_per_binary, 20.0);
        assert_eq!(a.max_cpu_s_per_binary, 30.0);
        assert_eq!((a.median_rss_mb, a.max_rss_mb), (300.0, 900.0));
        assert_eq!(
            s.by_arch["decompile"]["armv7"]["a"].cpu_ms_per_function,
            200.0
        );
        // the faster tool is listed first
        let text = render(&s);
        assert!(text.find("| b |").unwrap() < text.find("| a |").unwrap());
        // a line round-trips through JSON with the usage fields flattened
        let line = serde_json::to_string(&rows[0]).unwrap();
        assert!(line.contains("\"cpu_s\":10.0") && !line.contains("\"usage\""));
        assert_eq!(serde_json::from_str::<Timing>(&line).unwrap(), rows[0]);
    }
}

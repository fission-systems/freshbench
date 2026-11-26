//! Reuse of a pinned tool's outputs between sweeps.
//!
//! Decompiling is most of a sweep's time, and a tool whose version, adapter,
//! environment and input are unchanged prints the same text again. A job
//! given the artifacts of an earlier run takes a binary's output from them
//! instead of running the tool, only when the key of that output equals the
//! key this job computes. Scores are never reused: they are computed again
//! from the records, so a change to the scorer applies to reused outputs.
//!
//! The key is the SHA-256 of [`Parts`], which name everything that can
//! change an output:
//!
//! * the tool, its version string (`provenance::tool_version`: the pins) and
//!   the hash of its adapter files;
//! * the environment `ci/setup_tool.sh` recorded (interpreter or JVM version,
//!   a Python tool's installed packages, a built tool's binary);
//! * the hash of the harness sources that run the tool and build its records;
//! * the binary (build, project, name, SHA-256 of the stripped file);
//! * what was asked and under which limits (the requested addresses and names,
//!   the time limits, the binaries run at once; for the boundary track the
//!   variant and the input actually given).
//!
//! Fission is never reused. A result cut off by the job's own deadline, or one
//! the harness could not read, says nothing about the tool and is not reused;
//! a timeout under the per-call and per-binary limits is the tool's result and
//! is (it reproduced in 99% of binaries between two sweeps; the 1.4% of angr
//! binaries with a timeout took 42% of angr's time).

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::adapters::{failure_kind, Record, Usage};
use crate::manifest::sha256_bytes;
use crate::speed::Timing;

/// The version of the key's own layout: a change to it makes every key new.
pub const SCHEMA: &str = "1";

/// What identifies the tool and its setup in one job: the same for all its binaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub tool: String,
    pub tool_version: String,
    pub adapter_sha256: String,
    pub env_sha256: String,
}

/// Everything that can change one binary's output, as name -> value.
pub type Parts = BTreeMap<String, String>;

/// The key of `parts`: SHA-256 of their compact JSON with sorted names
/// (`ci/leaderboard_gate.py` computes the same).
pub fn key(parts: &Parts) -> String {
    sha256_bytes(
        serde_json::to_string(parts)
            .expect("a string map serializes")
            .as_bytes(),
    )
}

/// The sources that run a tool and turn what it prints into records.
const HARNESS: [(&str, &str); 4] = [
    ("adapters.rs", include_str!("adapters.rs")),
    ("boundaries.rs", include_str!("boundaries.rs")),
    ("evidence.rs", include_str!("evidence.rs")),
    ("reuse.rs", include_str!("reuse.rs")),
];

/// Hash of [`HARNESS`]: a change to how tools are run or read makes every key new.
pub fn harness_sha256() -> String {
    let mut all = Vec::new();
    for (name, text) in HARNESS {
        all.extend_from_slice(format!("{name}\0{}\0", text.len()).as_bytes());
        all.extend_from_slice(text.as_bytes());
    }
    sha256_bytes(&all)
}

/// Hash of every file under `dirs` (path relative to its directory, then
/// content), in sorted order. Build outputs (`target`, `__pycache__`) are
/// left out. A directory that does not exist is an error, not an empty hash.
pub fn files_sha256(dirs: &[PathBuf]) -> Result<String> {
    if dirs.is_empty() {
        bail!("no adapter directory to hash");
    }
    let mut all = Vec::new();
    for dir in dirs {
        let label = dir
            .file_name()
            .with_context(|| format!("{}: not a directory name", dir.display()))?
            .to_string_lossy()
            .into_owned();
        let mut files = Vec::new();
        collect(dir, dir, &mut files)?;
        files.sort();
        for (rel, path) in files {
            let data = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            all.extend_from_slice(format!("{label}/{rel}\0{}\0", data.len()).as_bytes());
            all.extend_from_slice(&data);
        }
    }
    Ok(sha256_bytes(&all))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            let name = e.file_name();
            if name != "target" && name != "__pycache__" {
                collect(root, &p, out)?;
            }
        } else {
            let rel = p.strip_prefix(root)?.to_string_lossy().into_owned();
            out.push((rel, p));
        }
    }
    Ok(())
}

/// The requested addresses (with the Thumb bit) and names, as one hash.
pub fn requests_sha256(funcs: &BTreeMap<u64, String>) -> String {
    let text: String = funcs.iter().map(|(a, n)| format!("{a:#x} {n}\n")).collect();
    sha256_bytes(text.as_bytes())
}

/// One binary of a job.
#[derive(Clone, Copy, Debug)]
pub struct Binary<'a> {
    pub build: &'a str,
    pub project: &'a str,
    pub name: &'a str,
    /// SHA-256 of the stripped binary
    pub stripped_sha256: &'a str,
}

fn base(ctx: &Context, track: &str, b: Binary, binary_timeout_s: u64, jobs: usize) -> Parts {
    [
        ("schema", SCHEMA.to_string()),
        ("track", track.to_string()),
        ("tool", ctx.tool.clone()),
        ("tool_version", ctx.tool_version.clone()),
        ("adapter_sha256", ctx.adapter_sha256.clone()),
        ("env_sha256", ctx.env_sha256.clone()),
        ("harness_sha256", harness_sha256()),
        ("build", b.build.to_string()),
        ("project", b.project.to_string()),
        ("binary", b.name.to_string()),
        ("stripped_sha256", b.stripped_sha256.to_string()),
        ("binary_timeout_s", binary_timeout_s.to_string()),
        ("jobs", jobs.max(1).to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// The parts of one binary's decompile output.
pub fn decompile_parts(
    ctx: &Context,
    b: Binary,
    funcs: &BTreeMap<u64, String>,
    binary_timeout_s: u64,
    function_timeout_s: u64,
    jobs: usize,
) -> Parts {
    let mut p = base(ctx, "decompile", b, binary_timeout_s, jobs);
    p.insert("requests_sha256".into(), requests_sha256(funcs));
    p.insert("function_timeout_s".into(), function_timeout_s.to_string());
    p
}

/// The parts of one binary's function list: `input_sha256` is the file the
/// tool was given (the stripped binary, or it without unwind tables).
pub fn boundary_parts(
    ctx: &Context,
    variant: &str,
    b: Binary,
    input_sha256: &str,
    binary_timeout_s: u64,
    jobs: usize,
) -> Parts {
    let mut p = base(ctx, "boundaries", b, binary_timeout_s, jobs);
    p.insert("variant".into(), variant.to_string());
    p.insert("input_sha256".into(), input_sha256.to_string());
    p
}

/// What a job says about where one binary's output came from; written beside
/// the results and gathered into `provenance.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub parts: Parts,
    /// whether a later job may take this output
    pub reusable: bool,
    /// the evaluate run that ran the tool, when this job did not
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reused_from: Option<String>,
    /// the key that output was recorded under
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
}

impl Entry {
    pub fn fresh(parts: Parts, reusable: bool) -> Entry {
        Entry {
            key: key(&parts),
            parts,
            reusable,
            reused_from: None,
            source_key: None,
        }
    }

    pub fn reused(parts: Parts, source_key: String, from_run: String) -> Entry {
        Entry {
            key: key(&parts),
            parts,
            reusable: true,
            reused_from: Some(from_run),
            source_key: Some(source_key),
        }
    }
}

/// Whether a binary's records are the tool's own result: something was
/// decompiled, and no function was cut off by the job's deadline or lost to an
/// output the harness could not read.
pub fn reusable(records: &[Record]) -> bool {
    records.iter().any(|r| r.code.is_some())
        && records.iter().all(|r| match r.error.as_deref() {
            None => true,
            Some(e) => !e.contains("job's time budget") && failure_kind(e) != "adapter",
        })
}

/// An earlier job's decompile output of one binary.
#[derive(Clone, Debug)]
pub struct Decompiled {
    pub records: Vec<Record>,
    /// what running the tool cost, as measured then
    pub timing: Option<Timing>,
    /// the evaluate run that ran the tool
    pub run: String,
}

/// An earlier job's function list of one binary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub entry: Entry,
    pub found: Vec<(u64, Option<u64>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// The run that measured an artifact: its `provenance.json`'s `evaluate_run`.
fn artifact_run(root: &Path) -> Result<String> {
    let f = root.join("provenance.json");
    let p: crate::provenance::Provenance = serde_json::from_slice(
        &std::fs::read(&f).with_context(|| format!("read {}", f.display()))?,
    )
    .with_context(|| format!("{}: not a provenance file", f.display()))?;
    Ok(p.evaluate_run)
}

fn lines<T: serde::de::DeserializeOwned>(f: &Path) -> Result<Vec<T>> {
    let text = std::fs::read_to_string(f).with_context(|| format!("read {}", f.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("{}: damaged line", f.display())))
        .collect()
}

fn read_records(f: &Path) -> Result<Vec<Record>> {
    let file = std::fs::File::open(f).with_context(|| format!("open {}", f.display()))?;
    std::io::BufReader::new(flate2::read::GzDecoder::new(file))
        .lines()
        .filter(|l| !l.as_ref().is_ok_and(|l| l.trim().is_empty()))
        .map(|l| {
            serde_json::from_str(&l?).with_context(|| format!("{}: damaged line", f.display()))
        })
        .collect()
}

/// The decompile outputs of earlier jobs, by key. `roots` are downloaded
/// `eval-*` artifacts. A damaged artifact or file is reported and skipped:
/// the tool then runs, which loses time but no result.
pub fn decompiled(roots: &[PathBuf]) -> BTreeMap<String, Decompiled> {
    let mut out = BTreeMap::new();
    for root in roots {
        if let Err(e) = decompiled_in(root, &mut out) {
            eprintln!("reuse: skipping {}: {e:#}", root.display());
        }
    }
    out
}

fn decompiled_in(root: &Path, out: &mut BTreeMap<String, Decompiled>) -> Result<()> {
    let run = artifact_run(root)?;
    let results = root.join("results");
    if !results.is_dir() {
        return Ok(());
    }
    let mut indexes = Vec::new();
    crate::rows::walk_sorted(&results, &mut |p| {
        if p.file_name().is_some_and(|n| n == "reuse.jsonl") {
            indexes.push(p.to_path_buf());
        }
    })?;
    for index in indexes {
        let unit = (|| -> Result<_> {
            let entries: Vec<Entry> = lines(&index)?;
            let records = read_records(&index.with_file_name("records.jsonl.gz"))?;
            let timing: Vec<Timing> = lines(&index.with_file_name("timing.jsonl"))?;
            Ok((entries, records, timing))
        })();
        let (entries, records, timing) = match unit {
            Ok(u) => u,
            Err(e) => {
                eprintln!("reuse: skipping {}: {e:#}", index.display());
                continue;
            }
        };
        for entry in entries {
            if !entry.reusable || key(&entry.parts) != entry.key {
                continue;
            }
            let Some(name) = entry.parts.get("binary") else {
                continue;
            };
            out.entry(entry.key.clone()).or_insert_with(|| Decompiled {
                records: records
                    .iter()
                    .filter(|r| &r.binary == name)
                    .cloned()
                    .collect(),
                timing: timing.iter().find(|t| &t.binary == name).cloned(),
                run: entry.reused_from.clone().unwrap_or_else(|| run.clone()),
            });
        }
    }
    Ok(())
}

impl Decompiled {
    /// Whether these records are exactly one per requested function.
    pub fn answers(&self, funcs: &BTreeMap<u64, String>) -> bool {
        let got: BTreeMap<u64, &String> = self
            .records
            .iter()
            .map(|r| (r.address, &r.function))
            .collect();
        self.records.len() == funcs.len() && got.into_iter().eq(funcs.iter().map(|(a, n)| (*a, n)))
    }
}

/// The function lists of earlier jobs, by key. `roots` are downloaded
/// `bound-*` artifacts, whose lists are in `found/*.jsonl`.
pub fn listed(roots: &[PathBuf]) -> BTreeMap<String, Listed> {
    let mut out = BTreeMap::new();
    for root in roots {
        let found = root.join("found");
        let files = (|| -> Result<(String, Vec<PathBuf>)> {
            let run = artifact_run(root)?;
            let mut files = Vec::new();
            if found.is_dir() {
                crate::rows::walk_sorted(&found, &mut |p| {
                    if p.extension().is_some_and(|e| e == "jsonl") {
                        files.push(p.to_path_buf());
                    }
                })?;
            }
            Ok((run, files))
        })();
        let (run, files) = match files {
            Ok(f) => f,
            Err(e) => {
                eprintln!("reuse: skipping {}: {e:#}", root.display());
                continue;
            }
        };
        for f in files {
            match lines::<Listed>(&f) {
                Ok(all) => {
                    for mut l in all {
                        if !l.entry.reusable || key(&l.entry.parts) != l.entry.key {
                            continue;
                        }
                        l.entry.reused_from.get_or_insert_with(|| run.clone());
                        out.entry(l.entry.key.clone()).or_insert(l);
                    }
                }
                Err(e) => eprintln!("reuse: skipping {}: {e:#}", f.display()),
            }
        }
    }
    out
}

/// Every [`Entry`] a job wrote under `dir`: `reuse.jsonl` beside decompile
/// results and `found/*.jsonl` of the boundary track. Damage is an error here:
/// this is what the job's own provenance claims.
pub fn entries_under(dir: &Path) -> Result<Vec<Entry>> {
    let mut decompile = Vec::new();
    let mut boundaries = Vec::new();
    crate::rows::walk_sorted(dir, &mut |p| {
        if p.file_name().is_some_and(|n| n == "reuse.jsonl") {
            decompile.push(p.to_path_buf());
        } else if p.extension().is_some_and(|e| e == "jsonl")
            && p.parent()
                .and_then(|d| d.file_name())
                .is_some_and(|d| d == "found")
        {
            boundaries.push(p.to_path_buf());
        }
    })?;
    let mut out = Vec::new();
    for f in decompile {
        out.extend(lines::<Entry>(&f)?);
    }
    for f in boundaries {
        out.extend(lines::<Listed>(&f)?.into_iter().map(|l| l.entry));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            tool: "ghidra".into(),
            tool_version: "ghidra 12.2".into(),
            adapter_sha256: "a".into(),
            env_sha256: "e".into(),
        }
    }

    fn binary<'a>() -> Binary<'a> {
        Binary {
            build: "gcc-O0",
            project: "p",
            name: "b",
            stripped_sha256: "s",
        }
    }

    fn funcs() -> BTreeMap<u64, String> {
        [(0x10, "f".to_string()), (0x21, "g".to_string())].into()
    }

    fn rec(address: u64, name: &str, code: Option<&str>, error: Option<&str>) -> Record {
        Record {
            binary: "b".into(),
            function: name.into(),
            address,
            code: code.map(str::to_string),
            error: error.map(str::to_string),
            evidence: None,
        }
    }

    /// Anything that can change an output changes the key.
    #[test]
    fn every_input_changes_the_key() {
        let base = |c: &Context, b: Binary, f: &BTreeMap<u64, String>, bt, ft, j| {
            key(&decompile_parts(c, b, f, bt, ft, j))
        };
        let k = base(&ctx(), binary(), &funcs(), 1800, 300, 3);
        assert_eq!(k, base(&ctx(), binary(), &funcs(), 1800, 300, 3));
        assert_eq!(k.len(), 64);
        let mut seen = vec![k];
        for change in 0..4 {
            let mut c = ctx();
            match change {
                0 => c.tool = "angr".into(),
                1 => c.tool_version = "ghidra 12.3".into(),
                2 => c.adapter_sha256 = "a2".into(),
                _ => c.env_sha256 = "e2".into(),
            }
            seen.push(base(&c, binary(), &funcs(), 1800, 300, 3));
        }
        for change in 0..4 {
            let mut b = binary();
            match change {
                0 => b.build = "gcc-O2",
                1 => b.project = "q",
                2 => b.name = "b2",
                _ => b.stripped_sha256 = "s2",
            }
            seen.push(base(&ctx(), b, &funcs(), 1800, 300, 3));
        }
        let mut thumb = funcs();
        thumb.remove(&0x10);
        thumb.insert(0x11, "f".into());
        let mut renamed = funcs();
        renamed.insert(0x10, "other".into());
        for f in [thumb, renamed, [(0x10, "f".to_string())].into()] {
            seen.push(base(&ctx(), binary(), &f, 1800, 300, 3));
        }
        seen.push(base(&ctx(), binary(), &funcs(), 900, 300, 3));
        seen.push(base(&ctx(), binary(), &funcs(), 1800, 60, 3));
        seen.push(base(&ctx(), binary(), &funcs(), 1800, 300, 4));
        // the boundary track: variant and the input actually given
        let l = |v, input| key(&boundary_parts(&ctx(), v, binary(), input, 1800, 3));
        seen.push(l("as-built", "s"));
        seen.push(l("no-unwind", "s"));
        seen.push(l("no-unwind", "s3"));
        let n = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), n);
    }

    /// The text the gate hashes: compact JSON, names sorted.
    #[test]
    fn the_key_is_the_hash_of_sorted_compact_json() {
        let p: Parts = [("b".into(), "2\n\"é".into()), ("a".into(), "1".into())].into();
        assert_eq!(
            key(&p),
            sha256_bytes("{\"a\":\"1\",\"b\":\"2\\n\\\"é\"}".as_bytes())
        );
    }

    #[test]
    fn an_adapter_edit_changes_its_hash() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("ghidra");
        let common = d.path().join("common");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&common).unwrap();
        std::fs::write(a.join("run"), "v1").unwrap();
        std::fs::write(common.join("lib.py"), "x").unwrap();
        let dirs = [a.clone(), common.clone()];
        let h = files_sha256(&dirs).unwrap();
        // build outputs are not the adapter
        std::fs::create_dir_all(a.join("target")).unwrap();
        std::fs::write(a.join("target/bin"), "b").unwrap();
        std::fs::create_dir_all(common.join("__pycache__")).unwrap();
        std::fs::write(common.join("__pycache__/c.pyc"), "c").unwrap();
        assert_eq!(files_sha256(&dirs).unwrap(), h);
        std::fs::write(a.join("run"), "v2").unwrap();
        let edited = files_sha256(&dirs).unwrap();
        assert_ne!(edited, h);
        std::fs::write(a.join("run"), "v1").unwrap();
        std::fs::write(common.join("lib.py"), "y").unwrap();
        assert_ne!(files_sha256(&dirs).unwrap(), h);
        std::fs::write(common.join("lib.py"), "x").unwrap();
        // a rename is a change
        std::fs::rename(a.join("run"), a.join("run2")).unwrap();
        assert_ne!(files_sha256(&dirs).unwrap(), h);
        assert!(files_sha256(&[d.path().join("none")]).is_err());
        assert!(files_sha256(&[]).is_err());
    }

    #[test]
    fn only_the_tools_own_results_are_reusable() {
        let ok = rec(0x10, "f", Some("int f(){}"), None);
        let with = |e: &str| vec![ok.clone(), rec(0x21, "g", None, Some(e))];
        assert!(reusable(std::slice::from_ref(&ok)));
        assert!(reusable(&with("no function at address")));
        assert!(reusable(&with("adapter failed: Segmentation fault")));
        assert!(reusable(&with(
            "timeout after 3600s: the binary's time budget is spent"
        )));
        assert!(reusable(&with("adapter failed: timeout after 300s")));
        assert!(!reusable(&with(
            "timeout after 17400s: the job's time budget is spent"
        )));
        assert!(!reusable(&with("adapter wrote no output")));
        assert!(!reusable(&with("output is not JSON")));
        // nothing decompiled: a broken setup must not be remembered
        assert!(!reusable(&[rec(
            0x10,
            "f",
            None,
            Some("adapter failed: no java")
        )]));
        assert!(!reusable(&[]));
    }

    fn artifact(root: &Path, run: &str, entry: &Entry, records: &[Record]) {
        std::fs::create_dir_all(root.join("results/ghidra/gcc-O0/p")).unwrap();
        let prov = serde_json::json!({"tool": "ghidra", "tool_version": "ghidra 12.2",
            "freshbench_commit": "c", "qualify_run": "q", "evaluate_run": run,
            "arch": "x86_64", "project": "p", "build": "all", "track": "decompile"});
        std::fs::write(root.join("provenance.json"), prov.to_string()).unwrap();
        let unit = root.join("results/ghidra/gcc-O0/p");
        crate::adapters::write_records(&unit.join("records.jsonl.gz"), records).unwrap();
        std::fs::write(
            unit.join("reuse.jsonl"),
            serde_json::to_string(entry).unwrap() + "\n",
        )
        .unwrap();
        let t = r#"{"tool":"ghidra","track":"decompile","project":"p","arch":"x86_64","compiler":"gcc","config":"O0","binary":"b","functions":2,"calls":1,"failed_calls":0,"jobs":3,"wall_s":7.5,"cpu_s":9.0,"max_rss_mb":100.0}"#;
        std::fs::write(unit.join("timing.jsonl"), format!("{t}\n")).unwrap();
    }

    /// A hit gives the records and their measured cost, under the run that ran
    /// the tool; any other key, an unreusable or damaged output is a miss.
    #[test]
    fn hit_and_miss() {
        let parts = decompile_parts(&ctx(), binary(), &funcs(), 1800, 300, 3);
        let k = key(&parts);
        let records = vec![
            rec(0x10, "f", Some("int f(){}"), None),
            rec(0x21, "g", None, Some("no function at address")),
        ];
        let d = tempfile::tempdir().unwrap();
        artifact(d.path(), "42", &Entry::fresh(parts.clone(), true), &records);
        let got = decompiled(&[d.path().to_path_buf()]);
        let hit = &got[&k];
        assert_eq!(hit.run, "42");
        assert_eq!(hit.records, records);
        assert_eq!(hit.timing.as_ref().unwrap().usage.wall_s, 7.5);
        assert!(hit.answers(&funcs()));
        assert!(!hit.answers(&[(0x10, "f".to_string())].into()));
        let other = decompile_parts(&ctx(), binary(), &funcs(), 900, 300, 3);
        assert!(!got.contains_key(&key(&other)));
        // a reused output keeps naming the run that measured it
        let d2 = tempfile::tempdir().unwrap();
        artifact(
            d2.path(),
            "99",
            &Entry::reused(parts.clone(), k.clone(), "42".into()),
            &records,
        );
        assert_eq!(decompiled(&[d2.path().to_path_buf()])[&k].run, "42");
        // not reusable, a key that does not match its parts, and damage: misses
        for (reusable, forge) in [(false, false), (true, true)] {
            let d = tempfile::tempdir().unwrap();
            let mut e = Entry::fresh(parts.clone(), reusable);
            if forge {
                e.parts.insert("jobs".into(), "9".into());
            }
            artifact(d.path(), "1", &e, &records);
            assert!(decompiled(&[d.path().to_path_buf()]).is_empty());
        }
        std::fs::write(
            d.path().join("results/ghidra/gcc-O0/p/records.jsonl.gz"),
            "x",
        )
        .unwrap();
        assert!(decompiled(&[d.path().to_path_buf()]).is_empty());
        assert!(decompiled(&[d.path().join("missing")]).is_empty());
    }

    #[test]
    fn function_lists_hit_and_miss() {
        let parts = boundary_parts(&ctx(), "as-built", binary(), "s", 1800, 3);
        let k = key(&parts);
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("found")).unwrap();
        std::fs::write(
            d.path().join("provenance.json"),
            r#"{"tool":"ghidra","tool_version":"v","freshbench_commit":"c","qualify_run":"q","evaluate_run":"7","arch":"x86_64","project":"p","build":"all","track":"boundaries"}"#,
        )
        .unwrap();
        let l = Listed {
            entry: Entry::fresh(parts, true),
            found: vec![(0x10, Some(0x20)), (0x30, None)],
            usage: Some(Usage {
                wall_s: 1.0,
                cpu_s: 2.0,
                max_rss_mb: 3.0,
            }),
        };
        std::fs::write(
            d.path().join("found/ghidra-as-built.jsonl"),
            serde_json::to_string(&l).unwrap() + "\n",
        )
        .unwrap();
        let got = listed(&[d.path().to_path_buf()]);
        assert_eq!(got[&k].found, l.found);
        assert_eq!(got[&k].entry.reused_from.as_deref(), Some("7"));
        assert_eq!(entries_under(d.path()).unwrap(), vec![l.entry.clone()]);
        std::fs::write(d.path().join("found/ghidra-as-built.jsonl"), "{bad\n").unwrap();
        assert!(listed(&[d.path().to_path_buf()]).is_empty());
        assert!(entries_under(d.path()).is_err());
    }
}

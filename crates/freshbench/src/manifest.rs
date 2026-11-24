//! Corpus manifests: every file of a milestone with its sha256 and provenance.
//!
//! JSON lines sorted by path. `kind` is binary, stripped, source, ground_truth,
//! license or other; binaries carry the compiler's exact version line, target
//! and flags. The manifest is committed to git; the files live on Hugging Face
//! at a pinned revision, and `verify` re-hashes a downloaded tree against it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rows::{split_build, walk_sorted};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Further provenance (compiler_version, flags, source_repo, source_commit, license, ...).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

pub const SKIP: [&str; 2] = ["README.md", ".gitattributes"];

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn sha256_bytes(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Kind and (project, arch, compiler, config) from a dataset-relative path.
pub fn classify(rel: &str) -> Entry {
    let parts: Vec<&str> = rel.split('/').collect();
    let kind = match parts[0] {
        "binaries" => "binary",
        "stripped" => "stripped",
        "sources" => "source",
        "truth" => "ground_truth",
        "projects" => "license",
        _ => "other",
    };
    let mut e = Entry {
        path: rel.to_string(),
        kind: kind.to_string(),
        ..Default::default()
    };
    match kind {
        "binary" | "stripped" | "source" | "ground_truth" if parts.len() > 2 => {
            if let Ok((a, c, o)) = split_build(parts[1]) {
                e.arch = Some(a);
                e.compiler = Some(c);
                e.config = Some(o);
                e.project = Some(parts[2].to_string());
            }
        }
        "license" if parts.len() > 1 => e.project = Some(parts[1].to_string()),
        _ => {}
    }
    e
}

/// Provenance merged into entries: per (project, build) for binaries, per project for everything.
#[derive(Default)]
pub struct Provenance {
    pub builds: BTreeMap<(String, String), BTreeMap<String, serde_json::Value>>,
    pub projects: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

pub fn build(root: &Path, prov: &Provenance) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut err = None;
    walk_sorted(root, &mut |p| {
        if err.is_some() {
            return;
        }
        let rel = p
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if SKIP.contains(&rel.as_str()) {
            return;
        }
        let mut e = classify(&rel);
        match (sha256_file(p), std::fs::metadata(p)) {
            (Ok(h), Ok(m)) => {
                e.sha256 = h;
                e.size = m.len();
            }
            (Err(x), _) => err = Some(x),
            (_, Err(x)) => err = Some(x.into()),
        }
        if let Some(proj) = e.project.clone() {
            if matches!(e.kind.as_str(), "binary" | "stripped") {
                let b = rel.split('/').nth(1).unwrap_or("").to_string();
                if let Some(x) = prov.builds.get(&(proj.clone(), b)) {
                    e.extra.extend(x.clone());
                }
            }
            if let Some(x) = prov.projects.get(&proj) {
                e.extra.extend(x.clone());
            }
        }
        out.push(e);
    })?;
    if let Some(e) = err {
        return Err(e);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

pub fn write(entries: &[Entry], path: &Path) -> Result<()> {
    let mut f = std::io::BufWriter::new(File::create(path)?);
    for e in entries {
        serde_json::to_writer(&mut f, e)?;
        f.write_all(b"\n")?;
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<Vec<Entry>> {
    let f = BufReader::new(File::open(path).with_context(|| format!("open {}", path.display()))?);
    let mut out = Vec::new();
    for line in f.lines() {
        let line = line?;
        if !line.trim().is_empty() {
            out.push(serde_json::from_str(&line)?);
        }
    }
    Ok(out)
}

/// Missing files, size/hash mismatches, and files not listed; sorted.
pub fn verify(root: &Path, entries: &[Entry]) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut listed = BTreeSet::new();
    for e in entries {
        listed.insert(e.path.clone());
        let full = root.join(&e.path);
        let Ok(meta) = std::fs::metadata(&full) else {
            problems.push(format!("missing {}", e.path));
            continue;
        };
        if meta.len() != e.size {
            problems.push(format!("size {}: {} != {}", e.path, meta.len(), e.size));
        } else if sha256_file(&full)? != e.sha256 {
            problems.push(format!("sha256 {}", e.path));
        }
    }
    walk_sorted(root, &mut |p| {
        let rel = p
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !listed.contains(&rel) && !SKIP.contains(&rel.as_str()) {
            problems.push(format!("unlisted {rel}"));
        }
    })?;
    problems.sort();
    Ok(problems)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Summary {
    pub files: BTreeMap<String, u64>,
    pub bytes: BTreeMap<String, u64>,
    pub total_bytes: u64,
    pub binaries_per_build: BTreeMap<String, u64>,
    pub compiler_versions: BTreeMap<String, String>,
    pub licenses: BTreeMap<String, u64>,
    pub projects: u64,
}

pub fn summarize(entries: &[Entry]) -> Summary {
    let mut s = Summary {
        files: BTreeMap::new(),
        bytes: BTreeMap::new(),
        total_bytes: 0,
        binaries_per_build: BTreeMap::new(),
        compiler_versions: BTreeMap::new(),
        licenses: BTreeMap::new(),
        projects: 0,
    };
    let mut lic: BTreeMap<String, String> = BTreeMap::new();
    let mut projects = BTreeSet::new();
    for e in entries {
        *s.files.entry(e.kind.clone()).or_default() += 1;
        *s.bytes.entry(e.kind.clone()).or_default() += e.size;
        s.total_bytes += e.size;
        let tag = |x: &Option<String>| x.clone().unwrap_or_default();
        if e.kind == "binary" {
            *s.binaries_per_build
                .entry(format!(
                    "{}/{}/{}",
                    tag(&e.arch),
                    tag(&e.compiler),
                    tag(&e.config)
                ))
                .or_default() += 1;
            if let Some(serde_json::Value::String(v)) = e.extra.get("compiler_version") {
                s.compiler_versions
                    .insert(format!("{}/{}", tag(&e.arch), tag(&e.compiler)), v.clone());
            }
        }
        if let Some(p) = &e.project {
            projects.insert(p.clone());
            if let Some(serde_json::Value::String(l)) = e.extra.get("license") {
                lic.insert(p.clone(), l.clone());
            }
        }
    }
    for l in lic.values() {
        *s.licenses.entry(l.clone()).or_default() += 1;
    }
    s.projects = projects.len() as u64;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tree(root: &Path) {
        for (rel, data) in [
            (
                "binaries/aarch64-gcc-O2/proj/a.out",
                &b"\x7fELF-aarch64"[..],
            ),
            ("binaries/gcc-O0/proj/a.out", b"\x7fELF-x86"),
            ("stripped/gcc-O0/proj/a.out", b"\x7fELF"),
            (
                "sources/gcc-O0/proj/0123.main.c",
                b"int main(void){return 0;}",
            ),
            ("truth/gcc-O0/proj/a.out.json", b"{}"),
            ("projects/proj/LICENSE", b"MIT"),
            ("README.md", b"card"),
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, data).unwrap();
        }
    }

    #[test]
    fn classify_paths() {
        let e = classify("binaries/armv7-clang-Os/p/x");
        assert_eq!(
            (
                e.kind.as_str(),
                e.arch.as_deref(),
                e.compiler.as_deref(),
                e.config.as_deref(),
                e.project.as_deref()
            ),
            (
                "binary",
                Some("armv7"),
                Some("clang"),
                Some("Os"),
                Some("p")
            )
        );
        assert_eq!(
            classify("binaries/gcc-O2-noinline/p/x").config.as_deref(),
            Some("O2-noinline")
        );
        assert_eq!(
            classify("binaries/gcc-O2-noinline/p/x").arch.as_deref(),
            Some("x86_64")
        );
        assert_eq!(classify("truth/gcc-O0/p/x.json").kind, "ground_truth");
        assert_eq!(classify("projects/p/LICENSE").project.as_deref(), Some("p"));
        assert_eq!(classify("weird/file").kind, "other");
    }

    #[test]
    fn build_verify_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("ds");
        make_tree(&root);
        let mut prov = Provenance::default();
        prov.builds.insert(
            ("proj".into(), "aarch64-gcc-O2".into()),
            [(
                "compiler_version".to_string(),
                json!("gcc (GCC) 16.2.0 target=aarch64-linux-gnu"),
            )]
            .into(),
        );
        prov.projects.insert(
            "proj".into(),
            [("license".to_string(), json!("MIT"))].into(),
        );
        let entries = build(&root, &prov).unwrap();
        assert!(entries.windows(2).all(|w| w[0].path < w[1].path));
        assert!(!entries.iter().any(|e| e.path == "README.md"));
        let a64 = entries
            .iter()
            .find(|e| e.path.starts_with("binaries/aarch64"))
            .unwrap();
        assert_eq!(
            a64.extra["compiler_version"],
            json!("gcc (GCC) 16.2.0 target=aarch64-linux-gnu")
        );
        let mf = d.path().join("m.jsonl");
        write(&entries, &mf).unwrap();
        assert_eq!(read(&mf).unwrap(), entries);
        assert!(verify(&root, &entries).unwrap().is_empty());
        let s = summarize(&entries);
        assert_eq!(
            (s.files["binary"], s.projects, s.licenses["MIT"]),
            (2, 1, 1)
        );
    }

    #[test]
    fn verify_reports_changes() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("ds");
        make_tree(&root);
        let entries = build(&root, &Provenance::default()).unwrap();
        std::fs::write(root.join("binaries/gcc-O0/proj/a.out"), b"\x7fELF-x87").unwrap();
        std::fs::remove_file(root.join("stripped/gcc-O0/proj/a.out")).unwrap();
        std::fs::write(root.join("extra.bin"), b"x").unwrap();
        assert_eq!(
            verify(&root, &entries).unwrap(),
            [
                "missing stripped/gcc-O0/proj/a.out",
                "sha256 binaries/gcc-O0/proj/a.out",
                "unlisted extra.bin"
            ]
        );
    }
}

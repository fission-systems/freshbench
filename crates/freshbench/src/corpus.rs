//! The corpus layout published per milestone, and the per-project prepared
//! trees it is assembled from.
//!
//! `freshbench prepare` writes one tree per (project, architecture):
//! `<cfg>/{binaries,stripped,manifest.json}` plus `sources/<project>/`. Build
//! directories there carry no architecture (`gcc-O0` on every arch). The
//! published corpus merges those trees into one root with arch-qualified build
//! names (`rows::build_name`):
//!
//! ```text
//! binaries/<build>/<project>/<binary>      unstripped, for grading only
//! stripped/<build>/<project>/<binary>      what decompilers see
//! truth/<build>/<project>/targets.json     selected functions (PreparedBuild)
//! sources/<arch>/<project>/<tu16>.<stem>.c preprocessed TUs (text differs per arch)
//! projects/<project>/<license files>
//! ```
//!
//! [`units`] reads either layout, so `decompile` and `rescore` work on a
//! single prepared tree and on the assembled corpus alike.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::prepare::PreparedBuild;
use crate::rows::build_name;

/// One (build, project): its selected functions and where its binaries are.
pub struct Unit {
    /// arch-qualified build name (`gcc-O0`, `aarch64-clang-Os`, ...)
    pub build: String,
    pub targets: PreparedBuild,
    /// directory holding the unstripped binaries, relative to the root
    pub binaries: String,
    /// directory holding the stripped binaries, relative to the root
    pub stripped: String,
}

fn sorted_dirs(p: &Path) -> Result<Vec<String>> {
    let mut v: Vec<String> = std::fs::read_dir(p)
        .with_context(|| format!("read_dir {}", p.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    Ok(v)
}

fn read_targets(p: &Path) -> Result<PreparedBuild> {
    serde_json::from_slice(&std::fs::read(p).with_context(|| format!("read {}", p.display()))?)
        .with_context(|| format!("parse {}", p.display()))
}

/// Every (build, project) of an assembled corpus (`truth/` present) or of a
/// prepared tree (`<cfg>/manifest.json`), sorted by (build, project).
pub fn units(root: &Path) -> Result<Vec<Unit>> {
    let mut out = Vec::new();
    let truth = root.join("truth");
    if truth.is_dir() {
        for build in sorted_dirs(&truth)? {
            for project in sorted_dirs(&truth.join(&build))? {
                let p = truth.join(&build).join(&project).join("targets.json");
                if !p.exists() {
                    continue;
                }
                let t = read_targets(&p)?;
                let b = build_name(&t.arch, &t.compiler, &t.config);
                if b != build || t.project != project {
                    bail!("{} describes {}/{}", p.display(), b, t.project);
                }
                out.push(Unit {
                    binaries: format!("binaries/{build}/{project}"),
                    stripped: format!("stripped/{build}/{project}"),
                    build: build.clone(),
                    targets: t,
                });
            }
        }
    } else {
        for dir in sorted_dirs(root)? {
            let p = root.join(&dir).join("manifest.json");
            if !p.exists() {
                continue;
            }
            let t = read_targets(&p)?;
            out.push(Unit {
                build: build_name(&t.arch, &t.compiler, &t.config),
                binaries: format!("{dir}/binaries"),
                stripped: format!("{dir}/stripped"),
                targets: t,
            });
        }
        out.sort_by(|a, b| (&a.build, &a.targets.project).cmp(&(&b.build, &b.targets.project)));
    }
    Ok(out)
}

/// Copy `src` to `dst` unless an identical file is already there; a different
/// file at `dst` is an error (two inputs disagree about one corpus path).
fn place(src: &Path, dst: &Path) -> Result<bool> {
    if dst.exists() {
        if std::fs::read(src)? == std::fs::read(dst)? {
            return Ok(false);
        }
        bail!("{} already exists with different content", dst.display());
    }
    std::fs::create_dir_all(dst.parent().unwrap())?;
    std::fs::copy(src, dst).with_context(|| format!("copy {}", src.display()))?;
    Ok(true)
}

fn files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v = Vec::new();
    if dir.is_dir() {
        crate::rows::walk_sorted(dir, &mut |p| v.push(p.to_path_buf()))?;
    }
    Ok(v)
}

/// Merge one prepared tree (and its project's license files) into the corpus
/// at `root`. Idempotent; returns the number of files written.
pub fn assemble(prepared: &Path, license: Option<&Path>, root: &Path) -> Result<usize> {
    let mut written = 0;
    let mut project: Option<(String, String)> = None;
    for u in units(prepared)? {
        let t = &u.targets;
        match &project {
            None => project = Some((t.project.clone(), t.arch.clone())),
            Some(p) if *p != (t.project.clone(), t.arch.clone()) => {
                bail!(
                    "{} mixes {:?} and {}/{}",
                    prepared.display(),
                    p,
                    t.project,
                    t.arch
                )
            }
            _ => {}
        }
        let (build, proj) = (&u.build, &t.project);
        for (kind, dir) in [("binaries", &u.binaries), ("stripped", &u.stripped)] {
            for bin in t.binaries.keys() {
                let src = prepared.join(dir).join(bin);
                let dst = root.join(kind).join(build).join(proj).join(bin);
                written += usize::from(place(&src, &dst)?);
            }
        }
        let mut moved: PreparedBuild = serde_json::from_value(serde_json::to_value(t)?)?;
        let old_prefix = format!("sources/{proj}/");
        let new_prefix = format!("sources/{}/{proj}/", t.arch);
        for b in moved.binaries.values_mut() {
            for f in &mut b.functions {
                let rest = f.source_path.strip_prefix(&old_prefix).with_context(|| {
                    format!("source path {} not under {old_prefix}", f.source_path)
                })?;
                let src = prepared.join(&f.source_path);
                f.source_path = format!("{new_prefix}{rest}");
                written += usize::from(place(&src, &root.join(&f.source_path))?);
            }
        }
        let tj = root
            .join("truth")
            .join(build)
            .join(proj)
            .join("targets.json");
        let body = serde_json::to_vec_pretty(&moved)?;
        if tj.exists() && std::fs::read(&tj)? != body {
            bail!("{} already exists with different content", tj.display());
        }
        if !tj.exists() {
            std::fs::create_dir_all(tj.parent().unwrap())?;
            std::fs::write(&tj, body)?;
            written += 1;
        }
    }
    let Some((proj, _)) = project else {
        bail!("{}: no prepared builds", prepared.display());
    };
    if let Some(lic) = license {
        for f in files(lic)? {
            let rel = f.strip_prefix(lic).unwrap();
            written += usize::from(place(&f, &root.join("projects").join(&proj).join(rel))?);
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::{PreparedBinary, SelectedFunction};

    fn prepared_tree(dir: &Path, arch: &str, tu_text: &str) {
        for cfg in ["gcc-O0", "clang-O2"] {
            let (compiler, config) = cfg.split_once('-').unwrap();
            let f = SelectedFunction {
                name: "f".into(),
                source: "main.c".into(),
                address: 0x1000,
                high_pc: None,
                selection_key: "k".into(),
                thumb: arch == "armv7",
                tu_sha256: "ab".into(),
                source_path: "sources/o__p/abcd.main.c".into(),
            };
            let pb = PreparedBuild {
                project: "o__p".into(),
                arch: arch.into(),
                compiler: compiler.into(),
                config: config.into(),
                flags: format!("-{config}"),
                compiler_version: format!("{compiler} 16 target={arch}"),
                binaries: [(
                    "p__a.elf".to_string(),
                    PreparedBinary {
                        sha256: "x".into(),
                        stripped_sha256: "y".into(),
                        strip_tool: "strip".into(),
                        functions: vec![f],
                    },
                )]
                .into(),
            };
            let d = dir.join(cfg);
            std::fs::create_dir_all(d.join("binaries")).unwrap();
            std::fs::create_dir_all(d.join("stripped")).unwrap();
            std::fs::write(d.join("binaries/p__a.elf"), format!("full {arch} {cfg}")).unwrap();
            std::fs::write(d.join("stripped/p__a.elf"), format!("strip {arch} {cfg}")).unwrap();
            std::fs::write(d.join("manifest.json"), serde_json::to_vec(&pb).unwrap()).unwrap();
        }
        std::fs::create_dir_all(dir.join("sources/o__p")).unwrap();
        std::fs::write(dir.join("sources/o__p/abcd.main.c"), tu_text).unwrap();
    }

    #[test]
    fn prepared_trees_assemble_into_arch_qualified_builds() {
        let d = tempfile::tempdir().unwrap();
        let (x86, arm, lic, root) = (
            d.path().join("x86"),
            d.path().join("arm"),
            d.path().join("lic"),
            d.path().join("corpus"),
        );
        prepared_tree(&x86, "x86_64", "int f(void){return 1;}");
        // same TU name, different text: the arch's headers differ
        prepared_tree(&arm, "armv7", "int f(void){return 2;}");
        std::fs::create_dir_all(&lic).unwrap();
        std::fs::write(lic.join("COPYING"), "MIT").unwrap();

        // a prepared tree names builds without the arch
        let pu = units(&arm).unwrap();
        assert_eq!(
            pu.iter().map(|u| u.build.as_str()).collect::<Vec<_>>(),
            ["armv7-clang-O2", "armv7-gcc-O0"]
        );

        assert!(assemble(&x86, Some(&lic), &root).unwrap() > 0);
        assert!(assemble(&arm, Some(&lic), &root).unwrap() > 0);
        // idempotent
        assert_eq!(assemble(&arm, Some(&lic), &root).unwrap(), 0);

        let u = units(&root).unwrap();
        let names: Vec<_> = u.iter().map(|u| u.build.as_str()).collect();
        assert_eq!(
            names,
            ["armv7-clang-O2", "armv7-gcc-O0", "clang-O2", "gcc-O0"]
        );
        let a = &u[1];
        let f = &a.targets.binaries["p__a.elf"].functions[0];
        assert!(f.thumb);
        assert_eq!(f.source_path, "sources/armv7/o__p/abcd.main.c");
        assert_eq!(
            std::fs::read_to_string(root.join(&f.source_path)).unwrap(),
            "int f(void){return 2;}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(&a.stripped).join("p__a.elf")).unwrap(),
            "strip armv7 gcc-O0"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("projects/o__p/COPYING")).unwrap(),
            "MIT"
        );

        // a second, different tree for the same (project, arch) is refused
        let other = d.path().join("other");
        prepared_tree(&other, "armv7", "int f(void){return 3;}");
        assert!(assemble(&other, None, &root).is_err());
    }
}

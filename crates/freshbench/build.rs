//! FRESHBENCH_SCORER_SRC: sha256 (16 hex) of every source file a score
//! depends on, so a change to the CFG builders, GED, parsing, types or the
//! vendored rust-joern changes `score::SCORER`, and with it every score-cache
//! key and the `scorer` of every row. Nobody has to remember to bump a version.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const SCORING: [&str; 9] = [
    "cfg.rs",
    "csyntax.rs",
    "dataset.rs",
    "dwarf.rs",
    "ged.rs",
    "joern.rs",
    "prepare.rs",
    "score.rs",
    "types.rs",
];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut es: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    es.sort();
    for p in es {
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut files: Vec<PathBuf> = SCORING.iter().map(|f| here.join("src").join(f)).collect();
    let joern = here.join("../rust-joern/src");
    rs_files(&joern, &mut files);
    files.push(here.join("../rust-joern/Cargo.toml"));
    files.push(here.join("../../Cargo.lock"));
    let mut h = Sha256::new();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let rel = f.strip_prefix(&here).unwrap_or(f);
        h.update(rel.to_string_lossy().as_bytes());
        h.update([0]);
        // line endings normalized so a Windows checkout hashes the same
        let text = std::fs::read(f).unwrap_or_default();
        let text: Vec<u8> = text.into_iter().filter(|&b| b != b'\r').collect();
        h.update(&text);
        h.update([0]);
    }
    println!("cargo:rerun-if-changed={}", joern.display());
    let hex: String = h.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("cargo:rustc-env=FRESHBENCH_SCORER_SRC={hex}");
}

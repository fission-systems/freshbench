//! How many selected functions are the same function: a source-level clone
//! audit of a corpus.
//!
//! Hashing compiled bytes does not find duplicates: the same source compiles to
//! different bytes in different binaries, and different sources can compile to
//! the same bytes. This module compares the selected functions' source
//! definitions instead, at three levels of the usual clone taxonomy:
//!
//! * `exact` (type 1): the same token stream, comments and layout aside, with
//!   the function's own name masked (a copy under another name is a copy);
//! * `renamed` (type 2): additionally every identifier replaced by its
//!   first-occurrence index (consistent renaming of variables, fields, types,
//!   callees);
//! * `shape` (type 2, blind): additionally every literal masked.
//!
//! Each level groups the distinct functions (project, source file, name) of the
//! corpus. A group of `k` functions holds `k - 1` redundant ones; a row is
//! redundant when its function is. Small functions collide at the blind level
//! by construction (`return x;`), which is why the token count is reported.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tree_sitter::Node;

use crate::csyntax;
use crate::manifest::hex;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Exact,
    Renamed,
    Shape,
}

pub const LEVELS: [Level; 3] = [Level::Exact, Level::Renamed, Level::Shape];

fn is_identifier(kind: &str) -> bool {
    matches!(
        kind,
        "identifier" | "field_identifier" | "type_identifier" | "statement_identifier"
    )
}

fn is_literal(kind: &str) -> bool {
    matches!(
        kind,
        "number_literal" | "string_content" | "escape_sequence" | "character" | "system_lib_string"
    )
}

/// The definition's leaf tokens (comments dropped), normalized for `level`.
pub fn tokens(def: Node, src: &str, level: Level) -> Vec<String> {
    let own = csyntax::function_name(def, src);
    let mut names: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::new();
    let mut stack = vec![def];
    while let Some(n) = stack.pop() {
        if n.kind() == "comment" {
            continue;
        }
        if n.child_count() == 0 {
            let t = &src[n.byte_range()];
            let k = n.kind();
            out.push(if is_identifier(k) && Some(t) == own {
                "$self".to_string()
            } else if level >= Level::Renamed && is_identifier(k) {
                let next = names.len();
                format!("$i{}", names.entry(t).or_insert(next))
            } else if level >= Level::Shape && is_literal(k) {
                "$lit".to_string()
            } else {
                t.to_string()
            });
            continue;
        }
        let mut c = n.walk();
        let kids: Vec<_> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    out
}

pub fn fingerprint(def: Node, src: &str, level: Level) -> String {
    let mut h = Sha256::new();
    for (i, t) in tokens(def, src, level).iter().enumerate() {
        if i > 0 {
            h.update(b" ");
        }
        h.update(t.as_bytes());
    }
    hex(&h.finalize())
}

/// One distinct selected function and how many rows it has.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Function {
    pub project: String,
    pub source: String,
    pub name: String,
    pub rows: usize,
    pub tokens: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Group {
    pub tokens: usize,
    pub projects: usize,
    pub members: Vec<Function>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LevelReport {
    pub level: Level,
    /// groups of two or more distinct functions
    pub groups: usize,
    pub cross_project_groups: usize,
    /// distinct functions minus groups (what deduplication would remove)
    pub redundant_functions: usize,
    pub redundant_rows: usize,
    /// redundant functions whose definition has at least 50 tokens
    pub redundant_functions_50_tokens: usize,
    /// the largest groups, then by token count
    pub top: Vec<Group>,
}

/// Selected functions that also occur in another corpus, per level.
#[derive(Debug, Serialize, Deserialize)]
pub struct Overlap {
    pub level: Level,
    pub functions: usize,
    pub rows: usize,
    pub functions_50_tokens: usize,
    /// up to `top` of them
    pub examples: Vec<Function>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub rows: usize,
    pub functions: usize,
    /// functions whose definition could not be read back (counted, not grouped)
    pub unreadable: usize,
    pub levels: Vec<LevelReport>,
    /// against `--against` (every definition in its .c/.i files)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub against: Vec<Overlap>,
}

/// Fingerprints at every level of every function defined in the .c/.i files
/// under `dir`.
pub fn corpus_fingerprints(dir: &Path) -> Result<[std::collections::HashSet<String>; 3]> {
    use rayon::prelude::*;
    let mut files = Vec::new();
    crate::rows::walk_sorted(dir, &mut |p| {
        if p.extension().is_some_and(|e| e == "c" || e == "i") {
            files.push(p.to_path_buf());
        }
    })?;
    let per: Vec<Vec<[String; 3]>> = files
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
                .map(|n| LEVELS.map(|l| fingerprint(n, &t, l)))
                .collect()
        })
        .collect();
    let mut out: [std::collections::HashSet<String>; 3] = Default::default();
    for fps in per.into_iter().flatten() {
        for (i, f) in fps.into_iter().enumerate() {
            out[i].insert(f);
        }
    }
    Ok(out)
}

/// Audit the corpus at `root` (assembled or prepared), optionally one arch,
/// optionally against another corpus's sources.
pub fn audit(
    root: &Path,
    arch: Option<&str>,
    top: usize,
    against: Option<&Path>,
) -> Result<Report> {
    use rayon::prelude::*;
    // (project, source, name) -> (first TU text path in sorted order, rows)
    type Key = (String, String, String);
    let mut fns: BTreeMap<Key, (String, usize)> = BTreeMap::new();
    let mut rows = 0;
    for u in crate::corpus::units(root)? {
        let t = &u.targets;
        if arch.is_some_and(|a| a != t.arch) {
            continue;
        }
        for b in t.binaries.values() {
            for f in &b.functions {
                rows += 1;
                let e = fns
                    .entry((t.project.clone(), f.source.clone(), f.name.clone()))
                    .or_insert_with(|| (f.source_path.clone(), 0));
                if f.source_path < e.0 {
                    e.0 = f.source_path.clone();
                }
                e.1 += 1;
            }
        }
    }
    let mut by_tu: BTreeMap<&str, Vec<(&Key, usize)>> = BTreeMap::new();
    for (k, (sp, n)) in &fns {
        by_tu.entry(sp.as_str()).or_default().push((k, *n));
    }
    type Fp = (Function, [String; 3]);
    let read: Vec<Option<Fp>> = by_tu
        .par_iter()
        .flat_map_iter(|(sp, members)| {
            let text = std::fs::read(root.join(sp))
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let tree = csyntax::parse(&text).ok();
            let defs = tree
                .as_ref()
                .map(|t| csyntax::definitions(t, &text))
                .unwrap_or_default();
            let out: Vec<Option<Fp>> = members
                .iter()
                .map(|((project, source, name), n)| {
                    let def = (*defs.get(name.as_str())?)?;
                    let f = Function {
                        project: project.clone(),
                        source: source.clone(),
                        name: name.clone(),
                        rows: *n,
                        tokens: tokens(def, &text, Level::Exact).len(),
                    };
                    Some((f, LEVELS.map(|l| fingerprint(def, &text, l))))
                })
                .collect();
            out
        })
        .collect();
    let unreadable = read.iter().filter(|r| r.is_none()).count();
    let read: Vec<Fp> = read.into_iter().flatten().collect();
    let mut levels = Vec::new();
    for (i, level) in LEVELS.into_iter().enumerate() {
        let mut groups: BTreeMap<&str, Vec<&Function>> = BTreeMap::new();
        for (f, fps) in &read {
            groups.entry(fps[i].as_str()).or_default().push(f);
        }
        let mut multi: Vec<Group> = groups
            .into_values()
            .filter(|g| g.len() > 1)
            .map(|g| {
                let mut projects: Vec<&str> = g.iter().map(|f| f.project.as_str()).collect();
                projects.sort();
                projects.dedup();
                Group {
                    tokens: g[0].tokens,
                    projects: projects.len(),
                    members: g.into_iter().cloned().collect(),
                }
            })
            .collect();
        // the first member (sorted by project, source, name) is kept, the rest are redundant
        let redundant = |g: &Group| g.members.iter().skip(1).cloned().collect::<Vec<_>>();
        let red: Vec<Function> = multi.iter().flat_map(redundant).collect();
        multi.sort_by(|a, b| {
            (b.members.len(), b.tokens, &a.members[0].name).cmp(&(
                a.members.len(),
                a.tokens,
                &b.members[0].name,
            ))
        });
        levels.push(LevelReport {
            level,
            groups: multi.len(),
            cross_project_groups: multi.iter().filter(|g| g.projects > 1).count(),
            redundant_functions: red.len(),
            redundant_rows: red.iter().map(|f| f.rows).sum(),
            redundant_functions_50_tokens: red.iter().filter(|f| f.tokens >= 50).count(),
            top: multi.into_iter().take(top).collect(),
        });
    }
    let mut overlap = Vec::new();
    if let Some(dir) = against {
        let other = corpus_fingerprints(dir)?;
        for (i, level) in LEVELS.into_iter().enumerate() {
            let hit: Vec<&Function> = read
                .iter()
                .filter(|(_, fps)| other[i].contains(&fps[i]))
                .map(|(f, _)| f)
                .collect();
            let mut examples: Vec<Function> = hit.iter().map(|f| (*f).clone()).collect();
            examples.sort_by_key(|f| std::cmp::Reverse(f.tokens));
            examples.truncate(top);
            overlap.push(Overlap {
                level,
                functions: hit.len(),
                rows: hit.iter().map(|f| f.rows).sum(),
                functions_50_tokens: hit.iter().filter(|f| f.tokens >= 50).count(),
                examples,
            });
        }
    }
    Ok(Report {
        rows,
        functions: fns.len(),
        unreadable,
        levels,
        against: overlap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(src: &str, name: &str, level: Level) -> String {
        let tree = csyntax::parse(src).unwrap();
        let defs = csyntax::definitions(&tree, src);
        fingerprint(defs[name].unwrap(), src, level)
    }

    #[test]
    fn levels_separate_copies_renamings_and_literals() {
        let src = "int a(int x) { /* c */ return x + 1; }\n\
                   int b(int x)\n{\n  return x + 1;\n}\n\
                   int c(int y) { return y + 1; }\n\
                   int d(int y) { return y + 2; }\n\
                   int e(int y) { return y * 2; }\n\
                   int f(int n) { return n ? f(n - 1) : 0; }\n\
                   int g(int n) { return n ? g(n - 1) : 0; }\n";
        // a copy under another name, layout and comments aside
        assert_eq!(fp(src, "a", Level::Exact), fp(src, "b", Level::Exact));
        // a renamed variable is a different text, the same renamed clone
        assert_ne!(fp(src, "a", Level::Exact), fp(src, "c", Level::Exact));
        assert_eq!(fp(src, "a", Level::Renamed), fp(src, "c", Level::Renamed));
        // another constant: blind only
        assert_ne!(fp(src, "c", Level::Renamed), fp(src, "d", Level::Renamed));
        assert_eq!(fp(src, "c", Level::Shape), fp(src, "d", Level::Shape));
        // another operator is never a clone
        assert_ne!(fp(src, "d", Level::Shape), fp(src, "e", Level::Shape));
        // recursion through its own name is the same function under another name
        assert_eq!(fp(src, "f", Level::Exact), fp(src, "g", Level::Exact));
        // consistent renaming only: `x + x` is not `x + y`
        let s2 = "int p(int x, int y) { return x + x; }\nint q(int x, int y) { return x + y; }\n";
        assert_ne!(fp(s2, "p", Level::Shape), fp(s2, "q", Level::Shape));
    }
}

//! Independent source CFG sizes, materialized once per corpus and scorer.
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::rows::RowKey;
use crate::{cfg, csyntax, dataset, manifest, score};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: RowKey,
    pub source: String,
    pub source_sha256: String,
    pub nodes: u32,
    pub edges: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Store {
    pub scorer: String,
    pub entries: Vec<Entry>,
}

pub type Index = BTreeMap<RowKey, Entry>;

/// Parse each translation unit once, build each selected source CFG once,
/// and fan its dimensions out to the corresponding binary functions.
pub fn build(root: &Path) -> Result<Store> {
    let targets = dataset::targets(root)?;
    anyhow::ensure!(!targets.is_empty(), "empty ground-truth population");
    let mut by_tu = BTreeMap::<&str, Vec<&dataset::Target>>::new();
    for t in &targets {
        by_tu.entry(&t.source_path).or_default().push(t);
    }
    let groups: Vec<_> = by_tu.into_iter().collect();
    let pool = dataset::big_stack_pool()?;
    let batches: Vec<Result<Vec<Entry>>> = pool.install(|| {
        groups
            .par_iter()
            .map(|(sp, ts)| {
                let bytes = std::fs::read(root.join(sp)).with_context(|| format!("read {sp}"))?;
                let sha = manifest::sha256_bytes(&bytes);
                let text = String::from_utf8_lossy(&bytes);
                let tree = csyntax::parse(&text)?;
                let defs = csyntax::definitions(&tree, &text);
                let mut dimensions = HashMap::<&str, (u32, u32)>::new();
                let mut entries = Vec::with_capacity(ts.len());
                for t in ts {
                    let name = t.key.function.as_str();
                    let (nodes, edges) = match dimensions.get(name) {
                        Some(&dims) => dims,
                        None => {
                            let def = defs.get(name).copied().flatten()
                                .with_context(|| format!("missing source definition {sp}:{name}"))?;
                            let graph = cfg::build(def, &text);
                            let dims = (graph.nodes() as u32, graph.edges.len() as u32);
                            dimensions.insert(name, dims);
                            dims
                        }
                    };
                    entries.push(Entry {
                        key: t.key.clone(),
                        source: t.source.clone(),
                        source_sha256: sha.clone(),
                        nodes,
                        edges,
                    });
                }
                Ok(entries)
            })
            .collect()
    });
    let mut entries = Vec::with_capacity(targets.len());
    for batch in batches {
        entries.extend(batch?);
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(Store {
        scorer: score::SCORER.into(),
        entries,
    })
}

/// Require the same scorer and exactly the corpus population, even if the
/// report is subsequently filtered by tool, clone group or build.
pub fn load(path: &Path, targets: &[dataset::Target]) -> Result<Index> {
    let store: Store = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(path)?,
    ))?;
    anyhow::ensure!(store.scorer == score::SCORER, "ground truth scorer mismatch");
    let mut index = Index::new();
    for entry in store.entries {
        anyhow::ensure!(entry.nodes > 0, "empty source CFG: {}", entry.key);
        let key = entry.key.clone();
        anyhow::ensure!(index.insert(key.clone(), entry).is_none(), "duplicate ground truth: {key}");
    }
    anyhow::ensure!(index.len() == targets.len(), "ground truth population size mismatch");
    for t in targets {
        let entry = index.get(&t.key).with_context(|| format!("missing ground truth: {}", t.key))?;
        anyhow::ensure!(entry.source == t.source, "ground truth source mismatch: {}", t.key);
    }
    Ok(index)
}

//! Where a result came from, and the page metadata built from it.
//!
//! Every evaluate job writes `provenance.json` beside its results: the tool
//! and the exact version it ran (as pinned in `ci/pins.env`, or the Fission
//! commit built), the freshbench commit, the corpus build and the run. The
//! results site takes its metadata from these files, never from the current
//! pins, and refuses artifacts that mix versions of one tool or corpus builds.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub tool: String,
    pub tool_version: String,
    pub freshbench_commit: String,
    pub qualify_run: String,
    pub evaluate_run: String,
    pub arch: String,
    pub project: String,
    /// an arch-qualified build, or `all`
    pub build: String,
    pub track: String,
}

/// `KEY=value` lines of a pins file (comments and blank lines skipped).
pub fn read_pins(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| {
            !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The version string a tool ran at: the Fission commit built, or the tool's
/// `version` rule in the registry (`ci/tools.json`) with each `{KEY}` replaced
/// by the pin `KEY`. The rules live in the registry, not here, so registering
/// a tool changes no scorer source and therefore no scorer identity.
pub fn tool_version(
    tool: &str,
    registry: &BTreeMap<String, crate::policy::ToolInfo>,
    pins: &BTreeMap<String, String>,
    fission_sha: &str,
) -> Result<String> {
    if tool == "fission" {
        return Ok(format!("fission {fission_sha}"));
    }
    let rule = registry
        .get(tool)
        .with_context(|| format!("tool {tool} is not in the registry"))?
        .version
        .as_deref()
        .with_context(|| format!("no version rule for tool {tool}"))?;
    let mut out = String::new();
    let mut rest = rule;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let j = after
            .find('}')
            .with_context(|| format!("{tool}: unclosed {{ in version rule {rule:?}"))?;
        let key = &after[..j];
        out.push_str(
            pins.get(key)
                .with_context(|| format!("{tool}: {key} is not in the pins"))?,
        );
        rest = &after[j + 1..];
    }
    if rest.contains('}') {
        bail!("{tool}: unmatched }} in version rule {rule:?}");
    }
    out.push_str(rest);
    Ok(out)
}

/// Page metadata from the artifacts' provenance files; an error when one tool
/// was measured at two versions or the artifacts come from two corpus builds.
pub fn meta(provs: &[Provenance], corpus: &str, notes: &[String]) -> Result<crate::report::Meta> {
    if provs.is_empty() {
        bail!("no provenance.json in the artifacts");
    }
    let mut by_tool: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for p in provs {
        by_tool
            .entry(p.tool.as_str())
            .or_default()
            .insert(p.tool_version.as_str());
    }
    let mixed: Vec<String> = by_tool
        .iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(t, v)| format!("{t}: {v:?}"))
        .collect();
    if !mixed.is_empty() {
        bail!(
            "one tool measured at several versions: {}",
            mixed.join("; ")
        );
    }
    let qual: BTreeSet<&str> = provs.iter().map(|p| p.qualify_run.as_str()).collect();
    if qual.len() != 1 {
        bail!("artifacts from several corpus builds: {qual:?}");
    }
    let mut pins: BTreeMap<String, String> = by_tool
        .iter()
        .map(|(t, v)| (t.to_string(), v.iter().next().unwrap().to_string()))
        .collect();
    pins.insert(
        "corpus build (qualify-arch run)".into(),
        qual.iter().next().unwrap().to_string(),
    );
    let commits: BTreeSet<String> = provs
        .iter()
        .map(|p| p.freshbench_commit.chars().take(12).collect())
        .collect();
    pins.insert(
        "freshbench commits".into(),
        commits.into_iter().collect::<Vec<_>>().join(", "),
    );
    let mut all_notes = notes.to_vec();
    all_notes.push(format!(
        "{} result artifacts; rows from different scorers or duplicated rows with different content are refused by the report",
        provs.len()
    ));
    Ok(crate::report::Meta {
        title: "FreshBench results".into(),
        corpus: corpus.into(),
        pins,
        notes: all_notes,
    })
}

/// Every `provenance.json` under `dir`.
pub fn load(dir: &Path) -> Result<Vec<Provenance>> {
    let mut out = Vec::new();
    let mut files = Vec::new();
    crate::rows::walk_sorted(dir, &mut |p| {
        if p.file_name().is_some_and(|n| n == "provenance.json") {
            files.push(p.to_path_buf())
        }
    })?;
    for f in files {
        out.push(
            serde_json::from_slice(&std::fs::read(&f)?)
                .with_context(|| format!("parse {}", f.display()))?,
        );
    }
    Ok(out)
}

/// Set `key=value` pairs in a JSON object file (created when missing).
pub fn set_status(path: &Path, pairs: &[String]) -> Result<()> {
    let mut m: BTreeMap<String, String> = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for kv in pairs {
        let (k, v) = kv
            .split_once('=')
            .with_context(|| format!("{kv}: expected key=value"))?;
        m.insert(k.into(), v.into());
    }
    std::fs::write(path, serde_json::to_vec_pretty(&m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(tool: &str, version: &str, qual: &str) -> Provenance {
        Provenance {
            tool: tool.into(),
            tool_version: version.into(),
            freshbench_commit: "0123456789abcdef".into(),
            qualify_run: qual.into(),
            evaluate_run: "9".into(),
            arch: "x86_64".into(),
            project: "p".into(),
            build: "all".into(),
            track: "decompile".into(),
        }
    }

    fn registry(rules: &[(&str, Option<&str>)]) -> BTreeMap<String, crate::policy::ToolInfo> {
        rules
            .iter()
            .map(|(t, v)| {
                (
                    t.to_string(),
                    crate::policy::ToolInfo {
                        repo: format!("o/{t}"),
                        llm: false,
                        version: v.map(str::to_string),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn pins_and_versions() {
        let pins = read_pins(
            "# c\nANGR_VERSION=10.0.7\nKUNA_TAG=v1\nKUNA_SHA256=ab\nlower=x\n\nUNLACE_SHA=ed9e\n",
        );
        assert_eq!(pins.len(), 4);
        let reg = registry(&[
            ("angr", Some("angr {ANGR_VERSION}")),
            ("kuna", Some("kuna {KUNA_TAG} sha256:{KUNA_SHA256}")),
            ("ghidra", Some("ghidra {GHIDRA_DIR}")),
            ("norule", None),
            ("open", Some("x {ANGR_VERSION")),
            ("close", Some("x ANGR_VERSION}")),
        ]);
        assert_eq!(
            tool_version("angr", &reg, &pins, "").unwrap(),
            "angr 10.0.7"
        );
        assert_eq!(
            tool_version("kuna", &reg, &pins, "").unwrap(),
            "kuna v1 sha256:ab"
        );
        assert_eq!(
            tool_version("fission", &reg, &pins, "24ec").unwrap(),
            "fission 24ec"
        );
        // a pin the rule names is missing
        assert!(tool_version("ghidra", &reg, &pins, "").is_err());
        assert!(tool_version("norule", &reg, &pins, "").is_err());
        assert!(tool_version("open", &reg, &pins, "").is_err());
        assert!(tool_version("close", &reg, &pins, "").is_err());
        assert!(tool_version("idk", &reg, &pins, "").is_err());
    }

    /// Every registered tool has a version rule that the checked-in pins
    /// satisfy, and the tools that had a rule in code before it moved to the
    /// registry still get the same version string.
    #[test]
    fn registered_tools_have_versions_from_the_checked_in_pins() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pins = read_pins(&std::fs::read_to_string(root.join("ci/pins.env")).unwrap());
        let reg: BTreeMap<String, crate::policy::ToolInfo> =
            serde_json::from_str(&std::fs::read_to_string(root.join("ci/tools.json")).unwrap())
                .unwrap();
        for tool in reg.keys() {
            let v = tool_version(tool, &reg, &pins, "abc").unwrap();
            assert!(v.starts_with(&format!("{tool} ")), "{tool}: {v}");
        }
        let pin = |k: &str| pins[k].clone();
        let before = [
            (
                "ghidra",
                format!(
                    "ghidra {} sha256:{}",
                    pin("GHIDRA_DIR"),
                    pin("GHIDRA_SHA256")
                ),
            ),
            ("angr", format!("angr {}", pin("ANGR_VERSION"))),
            (
                "kuna",
                format!("kuna {} sha256:{}", pin("KUNA_TAG"), pin("KUNA_SHA256")),
            ),
            (
                "borer",
                format!("borer {} sha256:{}", pin("BORER_TAG"), pin("BORER_SHA256")),
            ),
            ("unlace", format!("unlace {}", pin("UNLACE_SHA"))),
        ];
        for (tool, want) in before {
            assert_eq!(tool_version(tool, &reg, &pins, "").unwrap(), want);
        }
    }

    #[test]
    fn meta_refuses_mixed_versions_and_corpus_builds() {
        let ok = [
            p("angr", "angr 1", "q"),
            p("angr", "angr 1", "q"),
            p("kuna", "kuna 2", "q"),
        ];
        let m = meta(&ok, "M1", &["n".into()]).unwrap();
        assert_eq!(m.pins["angr"], "angr 1");
        assert_eq!(m.pins["corpus build (qualify-arch run)"], "q");
        assert_eq!(m.pins["freshbench commits"], "0123456789ab");
        assert_eq!(m.notes.len(), 2);
        let two = [p("angr", "angr 1", "q"), p("angr", "angr 2", "q")];
        assert!(format!("{:#}", meta(&two, "", &[]).unwrap_err()).contains("several versions"));
        let corp = [p("angr", "angr 1", "q"), p("kuna", "kuna 2", "r")];
        assert!(format!("{:#}", meta(&corp, "", &[]).unwrap_err()).contains("corpus builds"));
        assert!(meta(&[], "", &[]).is_err());
    }

    #[test]
    fn status_file_updates_in_place() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("status.json");
        set_status(&f, &["stage=clone".into(), "repo=a/b".into()]).unwrap();
        set_status(&f, &["stage=done".into(), "result=x=y".into()]).unwrap();
        let m: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        assert_eq!(m["stage"], "done");
        assert_eq!(m["repo"], "a/b");
        assert_eq!(m["result"], "x=y");
        assert!(set_status(&f, &["novalue".into()]).is_err());
    }
}

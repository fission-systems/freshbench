//! Which tools a published page may show.
//!
//! * Only decompilers listed in the tool registry (`ci/tools.json`), each with
//!   its upstream repository.
//! * Only maintained ones: the last commit on the upstream default branch is
//!   at most `max_idle_days` old at publication. An unmaintained project's
//!   numbers stay in the raw results but are not published, since nobody can
//!   act on them.
//! * No LLM-based decompilers in the ranking. The corpus is public, so a model
//!   may have seen it; LLM results would need their own track with its own
//!   contamination controls.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolInfo {
    /// upstream repository, `owner/name` on GitHub
    pub repo: String,
    /// the decompiler is (or uses) a large language model
    #[serde(default)]
    pub llm: bool,
}

/// Days since 1970-01-01 of a date that starts with `YYYY-MM-DD`.
pub fn days(date: &str) -> Result<i64> {
    let d = date
        .get(..10)
        .with_context(|| format!("{date}: not a date"))?;
    let p: Vec<i64> = d
        .split('-')
        .map(|x| x.parse::<i64>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("{date}: not YYYY-MM-DD"))?;
    let [y, m, dd] = p[..] else {
        bail!("{date}: not YYYY-MM-DD");
    };
    // days from civil (Howard Hinnant's algorithm)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + dd - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Ok(era * 146097 + doe - 719468)
}

/// The tools that may be published, and one note per tool left out.
pub fn publishable(
    tools: &BTreeSet<String>,
    registry: &BTreeMap<String, ToolInfo>,
    last_commit: &BTreeMap<String, String>,
    now: &str,
    max_idle_days: i64,
) -> Result<(BTreeSet<String>, Vec<String>)> {
    let today = days(now)?;
    let (mut keep, mut notes) = (BTreeSet::new(), Vec::new());
    for t in tools {
        let Some(info) = registry.get(t) else {
            notes.push(format!("{t}: not in the tool registry, not published"));
            continue;
        };
        if info.llm {
            notes.push(format!("{t}: LLM-based, not in this ranking"));
            continue;
        }
        let Some(last) = last_commit.get(t) else {
            notes.push(format!(
                "{t}: last upstream commit unknown ({}), not published",
                info.repo
            ));
            continue;
        };
        let idle = today - days(last)?;
        if idle > max_idle_days {
            notes.push(format!(
                "{t}: last upstream commit {} ({idle} days before {now}), unmaintained, not published",
                &last[..10.min(last.len())]
            ));
            continue;
        }
        keep.insert(t.clone());
    }
    Ok((keep, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(days("1970-01-01").unwrap(), 0);
        assert_eq!(days("2000-03-01").unwrap(), 11017);
        assert_eq!(
            days("2026-11-25T01:28:13Z").unwrap() - days("2025-11-25").unwrap(),
            365
        );
        assert!(days("2026-13").is_err());
    }

    #[test]
    fn maintained_registered_non_llm_only() {
        let reg: BTreeMap<String, ToolInfo> = [
            ("a", false),
            ("old", false),
            ("bot", true),
            ("nodate", false),
        ]
        .into_iter()
        .map(|(t, llm)| {
            (
                t.to_string(),
                ToolInfo {
                    repo: format!("o/{t}"),
                    llm,
                },
            )
        })
        .collect();
        let last: BTreeMap<String, String> = [
            ("a", "2026-11-20T10:00:00Z"),
            ("old", "2024-01-02T00:00:00Z"),
            ("bot", "2026-11-24T00:00:00Z"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let tools: BTreeSet<String> = ["a", "old", "bot", "nodate", "stranger"]
            .into_iter()
            .map(String::from)
            .collect();
        let (keep, notes) = publishable(&tools, &reg, &last, "2026-11-25", 365).unwrap();
        assert_eq!(keep.into_iter().collect::<Vec<_>>(), ["a"]);
        assert_eq!(notes.len(), 4);
        assert!(notes
            .iter()
            .any(|n| n.starts_with("old:") && n.contains("unmaintained")));
        assert!(notes
            .iter()
            .any(|n| n.starts_with("bot:") && n.contains("LLM")));
        assert!(notes
            .iter()
            .any(|n| n.starts_with("stranger:") && n.contains("registry")));
    }
}

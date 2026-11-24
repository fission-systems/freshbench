//! Scoring one decompiled function against its source function.

use serde::Serialize;

use crate::cfg;
use crate::csyntax::{self, Features};
use crate::ged;
use crate::manifest::sha256_bytes;
use crate::rows::GedScore;
use crate::taxonomy;

/// Package version plus a hash of the scoring sources (`build.rs`): any change
/// to how a score is computed gives a new scorer id.
pub const SCORER: &str = concat!(
    "freshbench-",
    env!("CARGO_PKG_VERSION"),
    "+",
    env!("FRESHBENCH_SCORER_SRC")
);

#[derive(Clone, Debug, Serialize)]
pub struct FunctionScore {
    pub ged: GedScore,
    pub tags: Vec<String>,
    pub code_sha256: String,
    pub source_features: Features,
    pub output_features: Features,
}

/// Why a function could not be scored.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Unscored {
    SourceMissing,
    OutputHasNoFunction,
}

/// Score the output text for `name` against the source function `src_fn_text`
/// (a translation unit containing exactly one definition of `name`).
pub fn score(source_tu: &str, output: &str, name: &str) -> Result<FunctionScore, Unscored> {
    let st = csyntax::parse(source_tu).map_err(|_| Unscored::SourceMissing)?;
    let defs = csyntax::definitions(&st, source_tu);
    let sdef = defs
        .get(name)
        .copied()
        .flatten()
        .ok_or(Unscored::SourceMissing)?;
    let ot = csyntax::parse(output).map_err(|_| Unscored::OutputHasNoFunction)?;
    let odef = csyntax::output_function(&ot, output, name).ok_or(Unscored::OutputHasNoFunction)?;
    let (sg, og) = (cfg::build(sdef, source_tu), cfg::build(odef, output));
    let d = ged::distance(&sg, &og);
    let (sf, of) = (
        csyntax::features(sdef, source_tu),
        csyntax::features(odef, output),
    );
    let (sn, on) = (sg.nodes() as u32, og.nodes() as u32);
    Ok(FunctionScore {
        ged: GedScore {
            value: d.value as f64,
            exact: d.exact,
            lower_bound: Some(d.lower_bound as f64),
            method: d.method.to_string(),
            source_nodes: Some(sn),
            source_edges: Some(sg.edges.len() as u32),
            output_nodes: Some(on),
            output_edges: Some(og.edges.len() as u32),
        },
        tags: taxonomy::tags(Some(sn), Some(on), Some(&sf), Some(&of)),
        code_sha256: sha256_bytes(output.as_bytes()),
        source_features: sf,
        output_features: of,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TU: &str = "#include <stdio.h>\nint g(int);\nstatic int f(int a, int b)\n{\n  if (a && b) return g(a);\n  return 0;\n}\nint main(void){ return f(1,2); }\n";

    #[test]
    fn equivalent_control_flow_is_perfect() {
        let out = "int FUN_00401000(int param_1, int param_2)\n{\n  int iVar1;\n  if (param_1 != 0) {\n    if (param_2 != 0) {\n      iVar1 = g(param_1);\n      return iVar1;\n    }\n  }\n  return 0;\n}\n";
        let s = score(TU, out, "f").unwrap();
        assert_eq!(s.ged.value, 0.0);
        assert!(s.ged.exact);
        assert_eq!(s.tags, ["lost_logical"]);
    }

    #[test]
    fn goto_tail_costs_and_is_tagged() {
        let out = "int f(int a, int b)\n{\n  if (a == 0) goto LAB_1;\n  if (b == 0) goto LAB_1;\n  return g(a);\nLAB_1:\n  return 0;\n}\n";
        let s = score(TU, out, "f").unwrap();
        assert_eq!(s.ged.value, 0.0, "same control flow, printed with gotos");
        assert!(s.tags.contains(&"extra_goto".to_string()));
    }

    #[test]
    fn unscored_cases() {
        assert_eq!(
            score(TU, "int f(void){return 0;}", "nope").unwrap_err(),
            Unscored::SourceMissing
        );
        assert_eq!(
            score(TU, "garbage ;;", "f").unwrap_err(),
            Unscored::OutputHasNoFunction
        );
    }

    #[test]
    fn same_text_same_score() {
        let out = "int f(int a,int b){ while (a) { a--; if (b) break; } return a; }";
        let a = score(TU, out, "f").unwrap();
        let b = score(TU, out, "f").unwrap();
        assert_eq!(a.ged, b.ged);
        assert_eq!(a.code_sha256, b.code_sha256);
    }
}

"""Failure taxonomy for one (tool, function) row.

A row gets one *outcome* and any number of *shape* tags. Tags compare the
decompiled text with the source function using the same lexical counts
(ctext.py) and the scorer's own GED node counts. They describe what the
output prints; they are not a verdict on correctness (FreshBench does not
measure correctness - see README).

Outcomes, in order of precedence:
  abstain      the tool does not support this target (architecture/format)
  no_output    the tool was run and printed nothing for the function
  unscored     output exists but the GED metric could not score it
  perfect      GED 0
  imperfect    GED > 0

Shape tags (only for rows with output):
  over_noded / under_noded   decompiled CFG has more / fewer nodes than source
  extra_goto                 more `goto` than the source
  extra_return               more `return` than the source
  lost_logical               fewer `&&`+`||` than the source
  extra_ternary              more `?:` than the source
  extra_calls / missing_calls  more / fewer call expressions than the source
  lost_switch                source has a switch, output has none
"""
from __future__ import annotations

from dataclasses import dataclass

from .ctext import Features

OUTCOMES = ("abstain", "no_output", "unscored", "perfect", "imperfect")
SHAPE_TAGS = (
    "over_noded", "under_noded", "extra_goto", "extra_return", "lost_logical",
    "extra_ternary", "extra_calls", "missing_calls", "lost_switch",
)


@dataclass(frozen=True)
class Classified:
    outcome: str
    tags: tuple[str, ...]

    @property
    def family(self) -> str:
        """One label for grouping: the outcome, or the sorted shape tags."""
        if self.outcome in ("abstain", "no_output", "unscored"):
            return self.outcome
        if not self.tags:
            return "shape_match" if self.outcome == "perfect" else "untagged"
        return "+".join(self.tags)


def outcome(row: dict) -> str:
    if row.get("abstain"):
        return "abstain"
    if not row.get("decompiled"):
        return "no_output"
    ged = row.get("ged")
    if ged is None:
        return "unscored"
    return "perfect" if ged == 0 else "imperfect"


def classify(row: dict, src: Features | None, out: Features | None) -> Classified:
    """Outcome and shape tags of a row. `src`/`out` may be None when the text is unavailable."""
    oc = outcome(row)
    if oc in ("abstain", "no_output"):
        return Classified(oc, ())
    tags: list[str] = []
    meta = row.get("ged_meta") or {}
    sn, dn = meta.get("source_nodes"), meta.get("decompiled_nodes")
    if sn is not None and dn is not None:
        if dn > sn:
            tags.append("over_noded")
        elif dn < sn:
            tags.append("under_noded")
    if src is not None and out is not None:
        if out.goto > src.goto:
            tags.append("extra_goto")
        if out.returns > src.returns:
            tags.append("extra_return")
        if out.logical_and + out.logical_or < src.logical_and + src.logical_or:
            tags.append("lost_logical")
        if out.ternary > src.ternary:
            tags.append("extra_ternary")
        if out.n_calls > src.n_calls:
            tags.append("extra_calls")
        elif out.n_calls < src.n_calls:
            tags.append("missing_calls")
        if src.switches > 0 and out.switches == 0:
            tags.append("lost_switch")
    return Classified(oc, tuple(t for t in SHAPE_TAGS if t in tags))

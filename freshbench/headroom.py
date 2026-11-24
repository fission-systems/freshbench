"""Where a decompiler can improve, measured against the other decompilers.

For one tool T and every function row:
  * best_other = the lowest GED any *other* tool scored on that row;
  * reachable  = another tool is perfect (GED 0) there and T is not;
  * gap        = T's GED minus best_other when T scored and is farther;
  * uncovered  = T printed nothing (or was unscored) where another tool scored.

"Reachable" is an existence proof: a decompiler already reaches the source's
shape on that row, from the same stripped binary. Gaps are grouped by size
band and by taxonomy tag; a row with several tags counts under each of them
(the tag totals overlap and are not summed). Every group lists example row
ids, worst first, ties broken by row id so the report is deterministic.

These are similarity gaps (GED), not correctness defects.
"""
from __future__ import annotations

import random
from collections import defaultdict
from dataclasses import dataclass, field

from .bands import BAND_LABELS, band
from .rows import RowKey


@dataclass
class Score:
    """One tool's result on one row, reduced to what headroom needs."""
    decompiled: bool
    ged: float | None
    source_nodes: int | None = None
    abstain: bool = False

    @property
    def scored(self) -> bool:
        return self.decompiled and self.ged is not None and not self.abstain


def score_of(row: dict) -> Score:
    meta = row.get("ged_meta") or {}
    return Score(bool(row.get("decompiled")), row.get("ged"), meta.get("source_nodes"),
                 bool(row.get("abstain")))


@dataclass
class Group:
    rows: int = 0
    perfect: int = 0
    reachable: int = 0
    gap: float = 0.0
    uncovered: int = 0
    examples: list = field(default_factory=list)  # (gap, row id)

    def add(self, *, perfect: bool, reachable: bool, gap: float, uncovered: bool, rid: str,
            keep: int) -> None:
        self.rows += 1
        self.perfect += perfect
        self.reachable += reachable
        self.gap += gap
        self.uncovered += uncovered
        if gap > 0 or reachable:
            self.examples.append((-gap, not reachable, rid))
            if len(self.examples) > 4 * keep:
                self.examples.sort()
                del self.examples[keep:]

    def top(self, keep: int) -> list[tuple[float, bool, str]]:
        return [(-g, not nr, rid) for g, nr, rid in sorted(self.examples)[:keep]]

    def as_dict(self, keep: int) -> dict:
        return {"rows": self.rows, "perfect": self.perfect, "reachable": self.reachable,
                "gap": self.gap, "uncovered": self.uncovered,
                "examples": [{"row": rid, "gap": g, "reachable": r} for g, r, rid in self.top(keep)]}


@dataclass
class Headroom:
    tool: str
    overall: Group
    by_band: dict[str, Group]
    by_tag: dict[str, Group]
    by_outcome: dict[str, Group]
    virtual_best_ged: float
    tool_ged_on_common: float
    common_rows: int

    def as_dict(self, keep: int = 5) -> dict:
        return {
            "tool": self.tool,
            "overall": self.overall.as_dict(keep),
            "by_band": {b: g.as_dict(keep) for b, g in self.by_band.items()},
            "by_tag": {t: g.as_dict(keep) for t, g in sorted(self.by_tag.items())},
            "by_outcome": {o: g.as_dict(keep) for o, g in sorted(self.by_outcome.items())},
            "virtual_best_ged_on_common": self.virtual_best_ged,
            "tool_ged_on_common": self.tool_ged_on_common,
            "common_rows": self.common_rows,
        }


def compute(table: dict[RowKey, dict[str, Score]], tool: str,
            tags: dict[RowKey, tuple[str, ...]] | None = None,
            outcomes: dict[RowKey, str] | None = None, keep: int = 5) -> Headroom:
    """Headroom of `tool` over a table row -> {tool name -> Score}."""
    overall = Group()
    by_band = {b: Group() for b in BAND_LABELS + ("unknown",)}
    by_tag: dict[str, Group] = defaultdict(Group)
    by_outcome: dict[str, Group] = defaultdict(Group)
    vbest = 0.0
    tool_common = 0.0
    common = 0
    for k in sorted(table):
        per = table[k]
        mine = per.get(tool)
        if mine is None or mine.abstain:
            continue
        others = [s.ged for t, s in per.items() if t != tool and s.scored]
        best_other = min(others) if others else None
        perfect = mine.scored and mine.ged == 0
        reachable = best_other == 0 and not perfect
        uncovered = not mine.scored and best_other is not None
        gap = 0.0
        if mine.scored and best_other is not None and mine.ged > best_other:
            gap = float(mine.ged - best_other)
        if mine.scored and best_other is not None:
            common += 1
            tool_common += mine.ged
            vbest += min(best_other, mine.ged)
        sn = mine.source_nodes
        if sn is None:
            sn = next((s.source_nodes for s in per.values() if s.source_nodes is not None), None)
        rid = k.as_id()
        kw = dict(perfect=perfect, reachable=reachable, gap=gap, uncovered=uncovered, rid=rid,
                  keep=keep)
        overall.add(**kw)
        by_band[band(sn)].add(**kw)
        for t in (tags or {}).get(k, ()):
            by_tag[t].add(**kw)
        if outcomes is not None and k in outcomes:
            by_outcome[outcomes[k]].add(**kw)
    if not by_band["unknown"].rows:
        del by_band["unknown"]
    return Headroom(tool, overall, by_band, dict(by_tag), dict(by_outcome), vbest, tool_common,
                    common)


def bootstrap_rate(per_project: dict[str, tuple[int, int]], resamples: int = 2000,
                   seed: int = 0) -> tuple[float, float, float]:
    """Rate (hits / rows) and its 95% interval from resampling projects.

    `per_project` maps project -> (hits, rows). Deterministic for a seed.
    """
    projects = sorted(per_project)
    if not projects:
        return (0.0, 0.0, 0.0)
    hits = sum(per_project[p][0] for p in projects)
    rows = sum(per_project[p][1] for p in projects)
    rate = hits / rows if rows else 0.0
    rng = random.Random(seed)
    samples = []
    for _ in range(resamples):
        h = n = 0
        for _ in projects:
            p = projects[rng.randrange(len(projects))]
            h += per_project[p][0]
            n += per_project[p][1]
        samples.append(h / n if n else 0.0)
    samples.sort()
    lo = samples[int(0.025 * (resamples - 1))]
    hi = samples[int(0.975 * (resamples - 1))]
    return (rate, lo, hi)

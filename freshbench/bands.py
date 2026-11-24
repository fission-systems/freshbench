"""Function-size bands and size-normalized GED.

GED grows with the size of the source CFG (nobody scores perfect above about
100 nodes), so totals are dominated by a few large functions. Every table is
also reported per band of source CFG nodes.
"""
from __future__ import annotations

import math

# (label, lowest node count, highest node count) - inclusive bounds
BANDS: tuple[tuple[str, int, float], ...] = (
    ("<6", 0, 5),
    ("6-20", 6, 20),
    ("21-50", 21, 50),
    ("51-100", 51, 100),
    (">100", 101, math.inf),
)
BAND_LABELS = tuple(b[0] for b in BANDS)


def band(source_nodes: int | None) -> str:
    """Band label for a source CFG node count; "unknown" when there is none."""
    if source_nodes is None or source_nodes < 0:
        return "unknown"
    for label, lo, hi in BANDS:
        if lo <= source_nodes <= hi:
            return label
    raise AssertionError("bands cover every non-negative count")


def normalized_ged(ged: float | None, source_nodes: int | None, source_edges: int | None) -> float | None:
    """GED divided by the source graph's size (nodes + edges), 0 for perfect.

    None when either side is missing. A source graph of size 0 cannot be
    normalized and returns None rather than dividing by zero.
    """
    if ged is None or source_nodes is None or source_edges is None:
        return None
    size = source_nodes + source_edges
    if size <= 0:
        return None
    return ged / size

"""Content-addressed scores: identical text always gets the identical score.

DecBench's GED is not deterministic on every input (the same decompiled text
has scored 0 in one run and 11 in another). FreshBench therefore

1. hashes every decompiled output (sha256 of the code) before scoring,
2. keys each score by (scorer identity, metric, ground-truth hash, output
   hash), so a text that was scored once is never scored again under the same
   scorer - two tools or two runs that print the same text get the same number,
3. rescores a fixed sample of rows in every run and fails the run if any score
   moved (`instability`), so a nondeterministic scorer is caught, not averaged.

The cache is an append-only JSON-lines file; the first value written for a
key wins, so a cache replayed from disk gives the same scores in any order.
"""
from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Callable, Iterable


def sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def cache_key(scorer_id: str, metric: str, truth_sha: str, output_sha: str) -> str:
    h = hashlib.sha256()
    for part in (scorer_id, metric, truth_sha, output_sha):
        h.update(part.encode("utf-8"))
        h.update(b"\0")
    return h.hexdigest()


class ScoreCache:
    def __init__(self, path: str | None = None):
        self.path = path
        self._scores: dict[str, object] = {}
        if path and os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                for line in f:
                    if line.strip():
                        rec = json.loads(line)
                        self._scores.setdefault(rec["key"], rec["value"])

    def __len__(self) -> int:
        return len(self._scores)

    def get(self, key: str):
        return self._scores.get(key)

    def __contains__(self, key: str) -> bool:
        return key in self._scores

    def score(self, scorer_id: str, metric: str, truth_sha: str, output_sha: str,
              compute: Callable[[], object]):
        """The cached value for the key, computing and recording it on first use."""
        key = cache_key(scorer_id, metric, truth_sha, output_sha)
        if key in self._scores:
            return self._scores[key]
        value = compute()
        self._scores[key] = value
        if self.path:
            with open(self.path, "a", encoding="utf-8") as f:
                f.write(json.dumps({"key": key, "value": value}, sort_keys=True) + "\n")
        return value


def rescore_sample(keys: Iterable[str], fraction: float = 0.01, salt: str = "freshbench-rescore-v1") -> list[str]:
    """A fixed, content-chosen subset of row ids (same rows every run)."""
    out = []
    limit = int(fraction * 2**32)
    for k in keys:
        h = int.from_bytes(hashlib.sha256((salt + k).encode()).digest()[:4], "big")
        if h < limit:
            out.append(k)
    return sorted(out)


def instability(first: dict[str, object], second: dict[str, object]) -> list[tuple[str, object, object]]:
    """Rows whose score differs between two scorings of the same text."""
    return sorted((k, first[k], second[k]) for k in first.keys() & second.keys() if first[k] != second[k])


def same_text_disagreements(rows: Iterable[dict], metric: str = "ged") -> list[tuple[str, list]]:
    """Ground truth + output pairs that received more than one score.

    `rows` need `code_sha256`, a ground-truth identity (`truth_sha` or, as in
    v1 results, the row's project/build/binary/function) and the metric.
    """
    seen: dict[tuple[str, str], set] = {}
    for r in rows:
        if not r.get("decompiled") or r.get(metric) is None:
            continue
        truth = r.get("truth_sha") or "/".join(
            str(r.get(f)) for f in ("project", "arch", "compiler", "config", "binary", "function"))
        seen.setdefault((truth, r["code_sha256"]), set()).add(r[metric])
    return sorted((f"{t}@{c[:12]}", sorted(v)) for (t, c), v in seen.items() if len(v) > 1)

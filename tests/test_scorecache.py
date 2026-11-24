import json

from freshbench.scorecache import (ScoreCache, cache_key, instability, rescore_sample,
                                   same_text_disagreements, sha256_text)


def test_key_separates_fields():
    assert cache_key("s", "ged", "ab", "c") != cache_key("s", "ged", "a", "bc")


def test_cache_computes_once_and_persists(tmp_path):
    p = tmp_path / "cache.jsonl"
    calls = []
    c = ScoreCache(str(p))
    v1 = c.score("s", "ged", "t", sha256_text("x"), lambda: calls.append(1) or 11)
    v2 = c.score("s", "ged", "t", sha256_text("x"), lambda: calls.append(1) or 0)
    assert (v1, v2, len(calls)) == (11, 11, 1)
    c2 = ScoreCache(str(p))
    assert c2.score("s", "ged", "t", sha256_text("x"), lambda: 0) == 11


def test_first_value_wins_on_replay(tmp_path):
    p = tmp_path / "c.jsonl"
    p.write_text(json.dumps({"key": "k", "value": 1}) + "\n" + json.dumps({"key": "k", "value": 2}) + "\n")
    assert ScoreCache(str(p)).get("k") == 1


def test_rescore_sample_is_stable_and_about_the_fraction():
    keys = [f"row{i}" for i in range(20000)]
    s1 = rescore_sample(keys, 0.01)
    assert s1 == rescore_sample(reversed(keys), 0.01)
    assert 120 < len(s1) < 280


def test_instability():
    assert instability({"a": 0, "b": 3}, {"a": 11, "b": 3, "c": 1}) == [("a", 0, 11)]


def test_same_text_disagreements():
    rows = [
        {"decompiled": True, "ged": 0.0, "code_sha256": "aa", "project": "p", "function": "f"},
        {"decompiled": True, "ged": 11.0, "code_sha256": "aa", "project": "p", "function": "f"},
        {"decompiled": True, "ged": 3.0, "code_sha256": "bb", "project": "p", "function": "f"},
        {"decompiled": False, "ged": None, "code_sha256": "aa", "project": "p", "function": "f"},
    ]
    bad = same_text_disagreements(rows)
    assert len(bad) == 1 and bad[0][1] == [0.0, 11.0]

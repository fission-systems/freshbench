from freshbench.headroom import Score, bootstrap_rate, compute
from freshbench.rows import RowKey


def k(fn, proj="p"):
    return RowKey(proj, "x86_64", "gcc", "O0", "bin", fn)


TABLE = {
    # another tool perfect, we are not: reachable, gap 4
    k("a"): {"me": Score(True, 4.0, 3), "x": Score(True, 0.0, 3), "y": Score(True, 9.0, 3)},
    # we are best: no gap
    k("b"): {"me": Score(True, 1.0, 30), "x": Score(True, 5.0, 30)},
    # we printed nothing, the other tool scored: uncovered
    k("c"): {"me": Score(False, None, 60), "x": Score(True, 2.0, 60)},
    # we are worse but nobody is perfect: gap only
    k("d"): {"me": Score(True, 10.0, 200), "x": Score(True, 7.0, 200)},
    # abstain rows are not ours to count
    k("e"): {"me": Score(False, None, 4, abstain=True), "x": Score(True, 0.0, 4)},
    # only we have the row
    k("f"): {"me": Score(True, 0.0, 2)},
    # both perfect: nothing to reach
    k("g"): {"me": Score(True, 0.0, 3), "x": Score(True, 0.0, 3)},
}


def test_overall():
    h = compute(TABLE, "me")
    o = h.overall
    assert (o.rows, o.perfect, o.reachable, o.uncovered) == (6, 2, 1, 1)
    assert o.gap == 4.0 + 3.0
    assert h.common_rows == 4
    assert h.tool_ged_on_common == 4 + 1 + 10
    assert h.virtual_best_ged == 0 + 1 + 7


def test_bands():
    h = compute(TABLE, "me")
    assert h.by_band["<6"].rows == 3  # a, f, g
    assert h.by_band["21-50"].rows == 1
    assert h.by_band["51-100"].uncovered == 1
    assert h.by_band[">100"].gap == 3.0
    assert "unknown" not in h.by_band


def test_tags_overlap_and_examples_are_deterministic():
    tags = {k("a"): ("extra_goto", "over_noded"), k("d"): ("over_noded",)}
    h = compute(TABLE, "me", tags=tags)
    assert h.by_tag["over_noded"].rows == 2
    assert h.by_tag["over_noded"].gap == 7.0
    ex = h.as_dict()["by_tag"]["over_noded"]["examples"]
    assert [e["row"] for e in ex] == ["p/gcc-O0/bin/a", "p/gcc-O0/bin/d"]
    assert ex[0] == {"row": "p/gcc-O0/bin/a", "gap": 4.0, "reachable": True}


def test_example_order_ties_by_id():
    t = {k(f"f{i}"): {"me": Score(True, 2.0, 3), "x": Score(True, 1.0, 3)} for i in (3, 1, 2)}
    ex = compute(t, "me", keep=2).as_dict(2)["overall"]["examples"]
    assert [e["row"] for e in ex] == ["p/gcc-O0/bin/f1", "p/gcc-O0/bin/f2"]


def test_bootstrap_deterministic_and_bounded():
    per = {"a": (5, 10), "b": (1, 10), "c": (9, 10)}
    r1 = bootstrap_rate(per, resamples=500, seed=1)
    r2 = bootstrap_rate(per, resamples=500, seed=1)
    assert r1 == r2
    rate, lo, hi = r1
    assert rate == 0.5
    assert 0.1 <= lo <= rate <= hi <= 0.9

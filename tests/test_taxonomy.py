from freshbench.ctext import features
from freshbench.taxonomy import classify, outcome

SRC = "int f(int a){ if (a && a > 2) return g(a); switch (a) { case 1: return 1; } return 0; }"


def row(**kw):
    base = {"decompiled": True, "ged": 3.0, "ged_meta": {"source_nodes": 5, "decompiled_nodes": 7}}
    base.update(kw)
    return base


def test_outcomes():
    assert outcome({"abstain": True}) == "abstain"
    assert outcome({"decompiled": False}) == "no_output"
    assert outcome({"decompiled": True, "ged": None}) == "unscored"
    assert outcome({"decompiled": True, "ged": 0}) == "perfect"
    assert outcome({"decompiled": True, "ged": 2.0}) == "imperfect"


def test_shape_tags():
    out = ("int f(int a){ if (a) { if (a > 2) goto L; } L: g(a); g(a); return a; return 0; "
           "return 1; return 2; x = a ? 1 : 2; }")
    c = classify(row(), features(SRC, "f"), features(out, "f"))
    assert c.outcome == "imperfect"
    assert c.tags == ("over_noded", "extra_goto", "extra_return", "lost_logical", "extra_ternary",
                      "extra_calls", "lost_switch")
    assert c.family == "+".join(c.tags)


def test_no_text_keeps_node_tags_only():
    c = classify(row(ged_meta={"source_nodes": 9, "decompiled_nodes": 4}), None, None)
    assert c.tags == ("under_noded",)


def test_no_output_has_no_tags():
    c = classify({"decompiled": False}, features(SRC), None)
    assert (c.outcome, c.tags, c.family) == ("no_output", (), "no_output")


def test_perfect_without_tags_is_shape_match():
    c = classify(row(ged=0, ged_meta={"source_nodes": 5, "decompiled_nodes": 5}), None, None)
    assert c.family == "shape_match"

import gzip
import json

from freshbench.cli import main

SRC_TU = "int f(int a) {\n  if (a && a > 1) return g(a);\n  return 0;\n}\n"


def row(tool, ged, sn=4, dn=4, decompiled=True, sha="s"):
    return {"project": "p", "compiler": "gcc", "config": "O0", "binary": "b", "function": "f",
            "tool": tool, "decompiled": decompiled, "ged": ged, "code_sha256": sha,
            "ged_meta": {"source_nodes": sn, "source_edges": 4, "decompiled_nodes": dn}}


def make(tmp_path, rows):
    (tmp_path / "results").mkdir()
    with open(tmp_path / "results/function_results.jsonl", "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    d = tmp_path / "results/me/gcc-O0/p"
    d.mkdir(parents=True)
    with gzip.open(d / "records.jsonl.gz", "wt") as f:
        f.write(json.dumps({"binary": "b", "function": "f",
                            "code": "int f(int a){ if (a) { if (a > 1) goto L; } return 0; L: return g(a); }"}) + "\n")
    (tmp_path / "sources/gcc-O0/p").mkdir(parents=True)
    (tmp_path / "sources/gcc-O0/p/x.c").write_text(SRC_TU)
    (tmp_path / "configs/O0").mkdir(parents=True)
    (tmp_path / "configs/O0/manifest.json").write_text(json.dumps({"binaries": [{
        "project": "p", "compiler": "gcc", "level": "O0", "binary": "b",
        "function_info": [{"name": "f", "source_path": "sources/gcc-O0/p/x.c"}]}]}))


def test_gaps_end_to_end(tmp_path, capsys):
    make(tmp_path, [row("me", 5.0, dn=6), row("other", 0.0)])
    out = tmp_path / "g.json"
    assert main(["gaps", str(tmp_path), "--tool", "me", "--json", str(out)]) == 0
    d = json.loads(out.read_text())
    assert d["overall"]["reachable"] == 1 and d["overall"]["gap"] == 5.0
    assert set(d["by_tag"]) == {"over_noded", "extra_goto", "lost_logical"}
    assert d["by_outcome"]["imperfect"]["rows"] == 1
    assert "reachable" in capsys.readouterr().out


def test_gaps_unknown_tool(tmp_path):
    make(tmp_path, [row("me", 1.0)])
    assert main(["gaps", str(tmp_path), "--tool", "nobody", "--no-text"]) == 2


def test_bands_and_determinism(tmp_path, capsys):
    make(tmp_path, [row("me", 0.0, sha="a"), row("other", 2.0, sn=30)])
    assert main(["bands", str(tmp_path)]) == 0
    text = capsys.readouterr().out
    assert "| me | <6 | 1 | 1 | 100.0% | 0.000 |" in text
    assert "| other | 21-50 | 1 | 0 |" in text
    assert main(["determinism", str(tmp_path)]) == 0


def test_determinism_flags_same_text_two_scores(tmp_path):
    make(tmp_path, [row("me", 0.0, sha="a"), row("other", 11.0, sha="a")])
    assert main(["determinism", str(tmp_path)]) == 1


def test_manifest_cli(tmp_path, capsys):
    make(tmp_path, [row("me", 0.0)])
    mf = tmp_path.parent / (tmp_path.name + "-m.jsonl")
    assert main(["manifest", "build", str(tmp_path), str(mf)]) == 0
    assert main(["manifest", "verify", str(tmp_path), str(mf)]) == 0
    (tmp_path / "sources/gcc-O0/p/x.c").write_text("changed")
    assert main(["manifest", "verify", str(tmp_path), str(mf)]) == 1
    assert main(["manifest", "summary", str(mf)]) == 0


def test_type_split():
    from freshbench.cli import type_split
    rows = [
        {"tool": "a", "type_match_perfect": True, "type_match_meta": {"tp": 2, "fp": 0, "fn": 0, "gt_vars": 2}},
        {"tool": "a", "type_match_perfect": False, "type_match_meta": {"tp": 1, "fp": 1, "fn": 2, "gt_vars": 4}},
        {"tool": "b", "type_match_perfect": False, "type_match_meta": {"tp": 0, "fp": 0, "fn": 3, "gt_vars": 3}},
        {"tool": "b", "type_match_meta": None},
    ]
    s = type_split(rows)
    assert s["a"]["rows"] == 2 and s["a"]["perfect"] == 1
    assert s["a"]["coverage"] == 4 / 6 and s["a"]["accuracy"] == 3 / 4
    assert s["b"]["coverage"] == 0 and s["b"]["accuracy"] is None


def test_determinism_flags_rescore_instability(tmp_path):
    r = row("me", 0.0, sha="a")
    r["ged_unstable"] = True
    make(tmp_path, [r])
    assert main(["determinism", str(tmp_path)]) == 1

import gzip
import json

import pytest

from freshbench.rows import RowKey, iter_rows, key_of, load_code, source_paths, split_build


def test_split_build():
    assert split_build("gcc-O2-noinline") == ("x86_64", "gcc", "O2-noinline")
    assert split_build("aarch64-clang-Os") == ("aarch64", "clang", "Os")
    assert split_build("armv7-gcc-O0") == ("armv7", "gcc", "O0")
    with pytest.raises(ValueError):
        split_build("O2")


def test_key_build_names():
    assert RowKey("p", "x86_64", "gcc", "O0", "b", "f").build == "gcc-O0"
    assert RowKey("p", "i686", "gcc", "O0", "b", "f").as_id() == "p/i686-gcc-O0/b/f"
    assert key_of({"project": "p", "compiler": "gcc", "config": "O0", "binary": "b", "function": "f"}).arch == "x86_64"


def write_gz(path, recs):
    path.parent.mkdir(parents=True, exist_ok=True)
    with gzip.open(path, "wt") as f:
        for r in recs:
            f.write(json.dumps(r) + "\n")


def test_dataset_tree(tmp_path):
    (tmp_path / "results").mkdir()
    (tmp_path / "results/function_results.jsonl").write_text(
        json.dumps({"project": "p", "compiler": "gcc", "config": "O0", "binary": "b", "function": "f",
                    "tool": "t"}) + "\n")
    write_gz(tmp_path / "results/t/aarch64-gcc-O0/p/records.jsonl.gz", [{"binary": "b", "function": "f", "code": "X"}])
    write_gz(tmp_path / "results/t/gcc-O0/p/records.jsonl.gz", [{"binary": "b", "function": "f", "code": "Y"}])
    assert len(list(iter_rows(str(tmp_path)))) == 1
    code = load_code(str(tmp_path), "t")
    assert code[RowKey("p", "aarch64", "gcc", "O0", "b", "f")] == "X"
    assert code[RowKey("p", "x86_64", "gcc", "O0", "b", "f")] == "Y"
    wanted = {RowKey("p", "x86_64", "gcc", "O0", "b", "f")}
    assert list(load_code(str(tmp_path), "t", wanted)) == list(wanted)
    cfg = tmp_path / "configs/O0"
    cfg.mkdir(parents=True)
    (cfg / "manifest.json").write_text(json.dumps({"binaries": [{
        "project": "p", "compiler": "gcc", "level": "O0", "binary": "b", "arch": "aarch64",
        "function_info": [{"name": "f", "source_path": "sources/aarch64-gcc-O0/p/x.c"}]}]}))
    assert source_paths(str(tmp_path)) == {RowKey("p", "aarch64", "gcc", "O0", "b", "f"): "sources/aarch64-gcc-O0/p/x.c"}


def test_run_tree(tmp_path):
    d = tmp_path / "p" / "clang-O1" / "t"
    d.mkdir(parents=True)
    (d / "rows.jsonl").write_text(json.dumps({"project": "p", "tool": "t"}) + "\n")
    write_gz(d / "records.jsonl.gz", [{"binary": "b", "function": "f", "code": "Z"}])
    assert len(list(iter_rows(str(tmp_path)))) == 1
    assert load_code(str(tmp_path), "t") == {RowKey("p", "x86_64", "clang", "O1", "b", "f"): "Z"}

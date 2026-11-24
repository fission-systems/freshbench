from freshbench.manifest import build_manifest, classify_path, read_manifest, summarize, verify, write_manifest


def make_tree(root):
    files = {
        "binaries/aarch64-gcc-O2/proj/a.out": b"\x7fELF-aarch64",
        "binaries/gcc-O0/proj/a.out": b"\x7fELF-x86",
        "stripped/gcc-O0/proj/a.out": b"\x7fELF",
        "sources/gcc-O0/proj/0123.main.c": b"int main(void){return 0;}",
        "pipeline_data/source_cfgs/gcc-O0/proj/a.out.json": b"{}",
        "projects/proj/LICENSE": b"MIT",
        "README.md": b"card",
    }
    for rel, data in files.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)


def test_classify_path():
    assert classify_path("binaries/armv7-clang-Os/p/x") == {
        "kind": "binary", "arch": "armv7", "compiler": "clang", "config": "Os", "project": "p"}
    assert classify_path("binaries/gcc-O2-noinline/p/x")["config"] == "O2-noinline"
    assert classify_path("binaries/gcc-O2-noinline/p/x")["arch"] == "x86_64"
    assert classify_path("pipeline_data/source_cfgs/gcc-O0/p/x.json")["kind"] == "source_cfg"
    assert classify_path("projects/p/LICENSE") == {"kind": "license", "project": "p"}
    assert classify_path("weird/file") == {"kind": "other"}


def test_build_verify_roundtrip(tmp_path):
    root = tmp_path / "ds"
    make_tree(root)
    prov = {("proj", "aarch64-gcc-O2"): {"compiler_version": "gcc (GCC) 16.2.0 target=aarch64-linux-gnu",
                                          "flags": "-O2"}}
    projects = {"proj": {"repo": "o/proj", "sha": "abc", "license": "MIT"}}
    entries = build_manifest(str(root), prov, projects)
    assert [e["path"] for e in entries] == sorted(e["path"] for e in entries)
    assert "README.md" not in {e["path"] for e in entries}
    a64 = next(e for e in entries if e["path"].startswith("binaries/aarch64"))
    assert a64["compiler_version"].endswith("target=aarch64-linux-gnu")
    assert all(e["license"] == "MIT" for e in entries)
    mf = tmp_path / "m.jsonl"
    write_manifest(entries, str(mf))
    assert read_manifest(str(mf)) == entries
    assert verify(str(root), entries) == []
    s = summarize(entries)
    assert s["files"]["binary"] == 2 and s["projects"] == 1 and s["licenses"] == {"MIT": 1}


def test_verify_reports_changes(tmp_path):
    root = tmp_path / "ds"
    make_tree(root)
    entries = build_manifest(str(root))
    (root / "binaries/gcc-O0/proj/a.out").write_bytes(b"\x7fELF-x87")  # same size, new bytes
    (root / "stripped/gcc-O0/proj/a.out").unlink()
    (root / "extra.bin").write_bytes(b"x")
    assert verify(str(root), entries) == [
        "missing stripped/gcc-O0/proj/a.out", "sha256 binaries/gcc-O0/proj/a.out", "unlisted extra.bin"]

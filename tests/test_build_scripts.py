"""The build scripts run inside the image; here we check what can be checked
without compilers: they parse, and both agree on the target table."""
import ast
import pathlib

BUILD = pathlib.Path(__file__).resolve().parents[1] / "build"


def literal(path, name):
    tree = ast.parse(path.read_text())
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and any(getattr(t, "id", None) == name for t in node.targets):
            v = node.value.value if isinstance(node.value, ast.Subscript) else node.value
            if isinstance(v, ast.Dict):
                return ast.literal_eval(v)
    raise AssertionError(f"{name} not found in {path}")


def test_scripts_parse():
    for p in ("fbcc.py", "build_project.py"):
        ast.parse((BUILD / p).read_text())


def test_arch_tables_agree():
    triplets = literal(BUILD / "fbcc.py", "TRIPLET")
    arches = literal(BUILD / "build_project.py", "ARCHES")
    assert set(triplets) == set(arches) == {"x86_64", "i686", "aarch64", "armv7"}
    for arch, (triplet, multiarch, elf_class, machine, cpu) in arches.items():
        if arch != "x86_64":
            assert triplets[arch] == triplet
        assert elf_class == (2 if arch in ("x86_64", "aarch64") else 1)
    assert arches["aarch64"][3] == 0xB7 and arches["armv7"][3] == 0x28 and arches["i686"][3] == 0x03


def test_native_build_unchanged_flags():
    # x86_64 adds no target flags, so v1 command lines are reproduced exactly
    src = (BUILD / "fbcc.py").read_text()
    assert 'if TRIPLET is None:' in src and "TGT = []" in src

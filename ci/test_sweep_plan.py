import gzip
import json
import tempfile
import unittest
from pathlib import Path

from ci.sweep_plan import arch_of, plan, population, tools_for


class SweepPlanTest(unittest.TestCase):
    def manifest(self, paths):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        p = Path(tmp.name) / "manifest.jsonl.gz"
        with gzip.open(p, "wt") as f:
            for path in paths:
                f.write(json.dumps({"path": path, "size": 1, "sha256": "x"}) + "\n")
        return p

    def test_arch_from_build_name(self):
        self.assertEqual(arch_of("gcc-O0"), "x86_64")
        self.assertEqual(arch_of("clang-O2-noinline"), "x86_64")
        self.assertEqual(arch_of("i686-gcc-O2"), "i686")
        self.assertEqual(arch_of("armv7-clang-Os"), "armv7")
        self.assertEqual(arch_of("aarch64-gcc-O3"), "aarch64")

    def test_population_reads_only_truth_target_files(self):
        m = self.manifest([
            "truth/gcc-O0/p/targets.json", "truth/gcc-O2/p/targets.json",
            "truth/i686-gcc-O0/q/targets.json", "binaries/gcc-O0/p/b",
            "truth/gcc-O0/p/other.json",
        ])
        self.assertEqual(population(m), {"i686": {"q": ["i686-gcc-O0"]},
                                         "x86_64": {"p": ["gcc-O0", "gcc-O2"]}})

    def test_tools_without_evidence_skip_the_decompile_track(self):
        reg = {"a": {}, "b": {"decompile": False}, "c": {"llm": True}, "d": {"sweep": False}}
        self.assertEqual(tools_for(reg, "decompile"), ["a"])
        self.assertEqual(tools_for(reg, "boundaries"), ["a", "b"])

    def test_plan_covers_every_target_once_per_track(self):
        pop = {a: {"big": [f"{a}-b{i}" for i in range(3)], "p": [f"{a}-b0"], "q": [f"{a}-b1"]}
               for a in ("x86_64", "i686", "aarch64", "armv7")}
        slices = plan(pop, {"a": {}, "b": {"decompile": False}}, ["big"])
        self.assertEqual(len(slices), 4 * 2 * 2)
        for arch in pop:
            for track in ("decompile", "boundaries"):
                mine = [s for s in slices if s["arch"] == arch and s["track"] == track]
                covered = sorted(p for s in mine for p in s["projects"])
                self.assertEqual(covered, ["big", "p", "q"])
                big = next(s for s in mine if s["projects"] == ["big"])
                self.assertEqual(big["builds"], pop[arch]["big"])
                rest = next(s for s in mine if s["projects"] != ["big"])
                self.assertEqual(rest["builds"], [""])
        self.assertTrue(all(s["tools"] == ["a"] for s in slices if s["track"] == "decompile"))

    def test_plan_refuses_an_unknown_split_project_and_a_missing_arch(self):
        pop = {a: {"p": ["b"]} for a in ("x86_64", "i686", "aarch64", "armv7")}
        with self.assertRaises(ValueError):
            plan(pop, {"a": {}}, ["nope"])
        del pop["armv7"]
        with self.assertRaises(ValueError):
            plan(pop, {"a": {}}, [])

    def test_plan_refuses_a_matrix_over_256_jobs(self):
        pop = {a: {f"p{i}": ["b"] for i in range(30)} for a in ("x86_64", "i686", "aarch64", "armv7")}
        with self.assertRaises(ValueError):
            plan(pop, {f"t{i}": {} for i in range(9)}, [])

    def test_checked_in_corpus_and_registry(self):
        root = Path(__file__).resolve().parent.parent
        reg = json.loads((root / "ci/tools.json").read_text())
        slices = plan(population(root / "corpus/m1/manifest.jsonl.gz"), reg, ["ComposersDesktop__CDP8"])
        self.assertEqual(len(slices), 16)
        for s in slices:
            self.assertLessEqual(len(s["tools"]) * len(s["projects"]) * len(s["builds"]), 256)


if __name__ == "__main__":
    unittest.main()

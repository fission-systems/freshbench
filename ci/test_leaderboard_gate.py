import json
import tempfile
import unittest
from pathlib import Path

from ci.leaderboard_gate import (BINARY_FIELDS, KEY_FIELDS, reuse_key, truth_facts, validate_boundaries,
                                 validate_reuse, validate_rows)


class LeaderboardGateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.rows = Path(self.tmp.name)
        self.keys = {("p", "x86_64", "gcc", "O0", "b", f) for f in ("f", "g")}
        self.registry = {"a": {}, "b": {}, "llm": {"llm": True}}

    def write(self, tool, keys, scorer="test"):
        rows = [dict(zip(KEY_FIELDS, key), tool=tool, scorer=scorer,
                     outcome="no_output") for key in sorted(keys)]
        (self.rows / f"{tool}.jsonl").write_text(
            "".join(json.dumps(row) + "\n" for row in rows))

    def test_complete_rows_including_failures(self):
        self.write("a", self.keys)
        self.write("b", self.keys)
        self.assertEqual(validate_rows(self.rows, self.keys, self.registry), {"a", "b"})

    def test_missing_row(self):
        self.write("a", self.keys)
        self.write("b", {min(self.keys)})
        with self.assertRaisesRegex(ValueError, "incomplete"):
            validate_rows(self.rows, self.keys, self.registry)

    def test_mixed_scorers(self):
        self.write("a", self.keys)
        self.write("b", self.keys, scorer="different")
        with self.assertRaisesRegex(ValueError, "scorer"):
            validate_rows(self.rows, self.keys, self.registry)

    def test_stray_row(self):
        self.write("a", self.keys)
        self.write("b", self.keys | {("p", "x86_64", "gcc", "O0", "b", "stray")})
        with self.assertRaisesRegex(ValueError, "unexpected target"):
            validate_rows(self.rows, self.keys, self.registry)

    def test_only_one_tool(self):
        self.write("a", self.keys)
        with self.assertRaisesRegex(ValueError, "at least two"):
            validate_rows(self.rows, self.keys, self.registry)

    def test_llm_excluded(self):
        self.write("a", self.keys)
        self.write("llm", self.keys)
        with self.assertRaisesRegex(ValueError, "LLM tool"):
            validate_rows(self.rows, self.keys, self.registry)


class BoundaryGateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bins = {("p", "x86_64", "gcc", "O0", b) for b in ("b1", "b2")}
        self.registry = {"a": {}, "b": {}, "llm": {"llm": True}}

    def write(self, run, tool, variant, bins, **extra):
        d = self.root / run / f"bound-{tool}" / "boundaries"
        d.mkdir(parents=True, exist_ok=True)
        rows = [dict(zip(BINARY_FIELDS, key), tool=tool, variant=variant, truth=1, **extra)
                for key in sorted(bins)]
        with (d / f"{tool}-{variant}.jsonl").open("a") as f:
            f.write("".join(json.dumps(r) + "\n" for r in rows))
        (d / "summary.json").write_text("{}")

    def both(self, tool, bins, run="1"):
        self.write(run, tool, "as-built", bins)
        self.write(run, tool, "no-unwind", bins)

    def test_complete_including_explicit_failures(self):
        self.both("a", self.bins)
        self.write("1", "b", "as-built", self.bins, error="timeout after 60s")
        self.write("1", "b", "no-unwind", self.bins)
        self.assertEqual(validate_boundaries([self.root], self.bins, self.registry), {"a", "b"})

    def test_a_cancelled_job_leaves_a_tool_incomplete(self):
        self.both("a", self.bins)
        self.both("b", {min(self.bins)})
        with self.assertRaisesRegex(ValueError, "incomplete boundary results: b as-built, b no-unwind"):
            validate_boundaries([self.root], self.bins, self.registry)

    def test_one_variant_missing(self):
        self.both("a", self.bins)
        self.write("1", "b", "as-built", self.bins)
        with self.assertRaisesRegex(ValueError, "b no-unwind"):
            validate_boundaries([self.root], self.bins, self.registry)

    def test_same_row_twice_is_kept_once_but_two_versions_are_refused(self):
        self.both("a", self.bins, run="1")
        self.both("a", self.bins, run="2")
        validate_boundaries([self.root], self.bins, self.registry)
        self.write("3", "a", "as-built", {min(self.bins)}, error="different")
        with self.assertRaisesRegex(ValueError, "disagree"):
            validate_boundaries([self.root], self.bins, self.registry)

    def test_stray_binary_and_llm_tool(self):
        self.both("a", self.bins | {("p", "x86_64", "gcc", "O0", "stray")})
        with self.assertRaisesRegex(ValueError, "unexpected binary"):
            validate_boundaries([self.root], self.bins, self.registry)

    def test_llm_tool_refused(self):
        self.both("llm", self.bins)
        with self.assertRaisesRegex(ValueError, "LLM tool"):
            validate_boundaries([self.root], self.bins, self.registry)

    def test_no_boundary_rows_is_allowed(self):
        self.assertEqual(validate_boundaries([], self.bins, self.registry), set())


class ReuseGateTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        record = {"stripped_sha256": "s", "functions": [
            {"address": 0x21, "name": "g"}, {"address": 0x10, "name": "f", "thumb": True}]}
        self.facts = {("armv7-gcc-O0", "p", "b"): truth_facts({}, record)}
        self.registry = {"ghidra": {}, "angr": {}, "fission": {}}
        self.parts = {"schema": "1", "track": "decompile", "tool": "ghidra", "tool_version": "ghidra 1",
                      "adapter_sha256": "a", "env_sha256": "e", "harness_sha256": "h",
                      "build": "armv7-gcc-O0", "project": "p", "binary": "b", "stripped_sha256": "s",
                      "binary_timeout_s": "1800", "function_timeout_s": "300", "jobs": "3",
                      **self.facts[("armv7-gcc-O0", "p", "b")]}

    def entry(self, reused_from="41", **change):
        parts = dict(self.parts, **change)
        e = {"parts": parts, "key": reuse_key(parts), "reusable": True}
        if reused_from:
            e.update(reused_from=reused_from, source_key=e["key"])
        return e

    def run_gate(self, *entries, tool="ghidra", job=0):
        d = self.root / "dl" / f"{job}"
        d.mkdir(parents=True, exist_ok=True)
        (d / "provenance.json").write_text(json.dumps(
            {"tool": tool, "tool_version": f"{tool} 1", "track": "decompile", "project": "p",
             "reuse": list(entries)}))
        return validate_reuse([self.root / "dl"], self.facts, self.registry)

    def refused(self, pattern, entry, **kw):
        with self.assertRaisesRegex(ValueError, pattern):
            self.run_gate(entry, **kw)

    def test_key_and_requests_match_the_harness(self):
        # reuse.rs: requests are "<addr with thumb bit> <name>" in address order
        self.assertEqual(self.parts["requests_sha256"],
                         __import__("hashlib").sha256(b"0x11 f\n0x21 g\n").hexdigest())
        # reuse.rs the_key_is_the_hash_of_sorted_compact_json
        self.assertEqual(reuse_key({"b": "2\n\"\u00e9", "a": "1"}),
                         __import__("hashlib").sha256(
                             '{"a":"1","b":"2\\n\\"\u00e9"}'.encode()).hexdigest())

    def test_fresh_and_exactly_matching_reuse_pass(self):
        self.assertEqual(self.run_gate(self.entry(reused_from=None), self.entry()), (2, 1))
        # jobs without reuse records (older runs) are not refused
        self.assertEqual(validate_reuse([], self.facts, self.registry), (0, 0))

    def test_reused_key_that_does_not_match_is_refused(self):
        e = self.entry()
        e["source_key"] = reuse_key(dict(self.parts, env_sha256="other"))
        self.refused("does not match", e)
        e = self.entry()
        del e["source_key"]
        self.refused("does not match", e)

    def test_key_must_be_the_hash_of_its_parts(self):
        e = self.entry()
        e["parts"]["env_sha256"] = "edited"
        self.refused("not the hash", e)

    def test_parts_must_describe_this_job_and_corpus(self):
        for change, pattern in [
            ({"tool": "angr"}, "another tool"), ({"tool_version": "ghidra 2"}, "another tool version"),
            ({"track": "boundaries"}, "another track"), ({"project": "q"}, "another project"),
            ({"binary": "c"}, "not an M1 binary"), ({"stripped_sha256": "x"}, "another binary file"),
            ({"requests_sha256": "x"}, "other requested functions"),
        ]:
            with self.subTest(change=change):
                self.refused(pattern, self.entry(**change))

    def test_fission_and_unusable_outputs_are_never_reused(self):
        self.refused("fission output was reused",
                     self.entry(tool="fission", tool_version="fission 1"), tool="fission")
        e = self.entry()
        e["reusable"] = False
        self.refused("marked unusable", e)
        self.refused("from no run", self.entry(reused_from="not-a-run"))

    def test_one_tool_one_adapter_and_harness(self):
        for field in ("adapter_sha256", "harness_sha256"):
            with self.subTest(field=field):
                self.run_gate(self.entry(), job=0)
                with self.assertRaisesRegex(ValueError, "two versions, adapters or harnesses"):
                    self.run_gate(self.entry(**{field: "other"}), job=1)
                (self.root / "dl" / "1" / "provenance.json").unlink()


if __name__ == "__main__":
    unittest.main()

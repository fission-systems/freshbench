import json
import tempfile
import unittest
from pathlib import Path

from ci.leaderboard_gate import BINARY_FIELDS, KEY_FIELDS, validate_boundaries, validate_rows


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


if __name__ == "__main__":
    unittest.main()

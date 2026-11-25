import json
import tempfile
import unittest
from pathlib import Path

from ci.leaderboard_gate import KEY_FIELDS, validate_rows


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


if __name__ == "__main__":
    unittest.main()

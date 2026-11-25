"""Every glob in .github/labeler.yml matches a tracked file, and every label
it assigns is a managed area:* label in .github/labels.json.

Run from the repository root: python -m unittest ci.test_labeler
"""

import json
import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def labeler_rules(text: str) -> dict[str, list[str]]:
    """label -> globs, for the changed-files/any-glob-to-any-file form used here."""
    rules: dict[str, list[str]] = {}
    label = None
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if not line.startswith(" "):
            label = line.rstrip(":").strip()
            rules[label] = []
        elif line.strip().startswith("- ") and not line.strip().endswith(":"):
            rules[label].append(line.strip()[2:].strip().strip('"'))
    return rules


def glob_regex(glob: str) -> re.Pattern:
    """minimatch semantics for the subset used (dot: true): `**/` is zero or
    more directories, a trailing `**` crosses /, `*` does not."""
    out, i = "", 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out += "(?:.*/)?"
            i += 3
        elif glob.startswith("**", i):
            out += ".*"
            i += 2
        elif glob[i] == "*":
            out += "[^/]*"
            i += 1
        else:
            out += re.escape(glob[i])
            i += 1
    return re.compile(out + r"\Z")


class LabelerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.rules = labeler_rules((ROOT / ".github/labeler.yml").read_text())
        cls.files = subprocess.check_output(["git", "ls-files"], cwd=ROOT, text=True).splitlines()
        cls.labels = {item["name"] for item in json.loads((ROOT / ".github/labels.json").read_text())}

    def test_the_parser_reads_every_rule(self):
        self.assertGreaterEqual(len(self.rules), 11)
        self.assertIn("crates/freshbench/**", self.rules["area:scorer"])
        self.assertIn("ci/pins.env", self.rules["area:pins"])
        self.assertTrue(all(self.rules.values()), "a label with no glob")

    def test_every_glob_matches_a_tracked_file(self):
        for label, globs in self.rules.items():
            for glob in globs:
                pattern = glob_regex(glob)
                self.assertTrue(any(pattern.match(f) for f in self.files), f"{label}: {glob} matches nothing")

    def test_glob_semantics(self):
        # Positive and negative controls for the matcher the check relies on.
        self.assertTrue(glob_regex("crates/freshbench/**").match("crates/freshbench/src/ged.rs"))
        self.assertFalse(glob_regex("crates/freshbench/**").match("crates/freshbench-x/src/lib.rs"))
        self.assertTrue(glob_regex("**/*.md").match("README.md"))
        self.assertTrue(glob_regex("**/*.md").match("corpus/m1/README.md"))
        self.assertFalse(glob_regex("**/*.md").match("ci/pins.env"))
        self.assertTrue(glob_regex("crates/freshbench/src/report*").match("crates/freshbench/src/report.css"))
        self.assertFalse(glob_regex("crates/freshbench/src/report*").match("crates/freshbench/src/rows.rs"))
        self.assertFalse(any(glob_regex("crates/no-such-crate/**").match(f) for f in self.files))

    def test_only_managed_area_labels(self):
        for label in self.rules:
            self.assertTrue(label.startswith("area:"), f"{label}: the labeler assigns area:* only")
            self.assertIn(label, self.labels, f"{label} is not in .github/labels.json")
        self.assertNotIn("risk:semantic", self.rules)

    def test_catalog_is_well_formed(self):
        catalog = json.loads((ROOT / ".github/labels.json").read_text())
        names = [item["name"].casefold() for item in catalog]
        self.assertEqual(len(names), len(set(names)), "duplicate label name")
        for item in catalog:
            self.assertRegex(item["color"], r"\A[0-9a-f]{6}\Z", item["name"])
            self.assertLessEqual(len(item["description"]), 100, item["name"])


if __name__ == "__main__":
    unittest.main()

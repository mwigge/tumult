#!/usr/bin/env python3
"""Static checks that the local gates are the gates of CI.

`make precommit` runs the checks of the CI jobs, every crate reads the
workspace lints, and every script test runs in CI.
"""

import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CI = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
MAKEFILE = (ROOT / "Makefile").read_text(encoding="utf-8")
# The CI jobs that `make precommit` runs too.
MIRRORED_JOBS = ("fmt", "test", "demo-crates", "deny", "docs-check")
# Steps that set up a runner rather than check the code.
SETUP = (
    "cargo install ",
    "sudo ",
    "df ",
    "npm --prefix web ci",
    "npx playwright install ",
    "curl ",
    "echo ",
    "unzip ",
)


def job(name):
    """The text of one job of ci.yml."""
    match = re.search(rf"^  {re.escape(name)}:\n(.*?)(?=^  \S|\Z)", CI, re.M | re.S)
    assert match, f"no job {name} in ci.yml"
    return match[1]


def commands(text):
    """The shell commands of the `run:` steps of a job, one per entry."""
    found = re.findall(r"run: (?!\|)(.+)", text)
    for block in re.findall(r"run: \|\n((?:          .*\n)+)", text):
        found += block.replace("\\\n", " ").splitlines()
    out = []
    for line in found:
        for command in line.split("&&"):
            command = re.sub(r'^(\w+=("[^"]*"|\S+)\s+)+', "", " ".join(command.split()))
            # CI reads the base of a pull request from the event; precommit
            # uses the default base of the script.
            command = re.sub(r' "[^"]*\$\{[^}]*\}[^"]*"$', "", command)
            if not command.startswith(SETUP):
                out.append(command)
    return out


def recipe(target, seen=()):
    """The recipe of a make target with the targets it runs, `$(MAKE) x`
    written `make x`."""
    match = re.search(rf"^{re.escape(target)}:(.*)\n((?:\t.*\n)*)", MAKEFILE, re.M)
    assert match, f"no target {target} in the Makefile"
    text = match[2].replace("$(MAKE)", "make")
    called = match[1].split() + re.findall(r"^\tmake (\S+)", text, re.M)
    for name in called:
        if name not in seen:
            text += recipe(name, (*seen, target, name))
    return text


class Precommit(unittest.TestCase):
    def test_precommit_runs_every_check_of_the_mirrored_ci_jobs(self):
        text = recipe("precommit")
        for name in MIRRORED_JOBS:
            for command in commands(job(name)):
                with self.subTest(job=name, command=command):
                    self.assertTrue(command in text, "not in make precommit")

    def test_precommit_mutates_the_changed_lines_and_checks_the_prose(self):
        text = recipe("precommit")
        self.assertIn("\tmake mutants\n", text)
        self.assertIn("scripts/mutants.sh origin/main", text)
        self.assertIn("\tpython3 scripts/check-prose.py\n", text)

    def test_dependencies_are_audited_by_cargo_deny_alone(self):
        # deny.toml holds the documented exceptions; a second audit tool with
        # its own list would disagree with CI.
        self.assertNotIn("cargo audit", recipe("precommit"))
        self.assertFalse((ROOT / ".cargo/audit.toml").exists())


class Prose(unittest.TestCase):
    def test_ci_checks_the_prose_of_the_lines_a_pull_request_adds(self):
        docs = job("docs-check")
        self.assertIn("fetch-depth: 0", docs)
        self.assertIn("BASE_REF: ${{ github.base_ref }}", docs)
        self.assertIn(
            'run: python3 scripts/check-prose.py "origin/${BASE_REF:-main}"', docs
        )


class ScriptTests(unittest.TestCase):
    def test_every_script_test_runs_in_ci(self):
        quality = job("scripts-quality")
        for test in sorted((ROOT / "scripts").glob("test-*.py")):
            with self.subTest(test=test.name):
                self.assertIn(f"run: python3 scripts/{test.name}\n", quality)


def manifest(path):
    return tomllib.loads(path.read_text(encoding="utf-8"))


class Lints(unittest.TestCase):
    def test_every_member_crate_reads_the_workspace_lints(self):
        members = manifest(ROOT / "Cargo.toml")["workspace"]["members"]
        self.assertGreater(len(members), 30)
        missing = [
            member
            for member in members
            if manifest(ROOT / member / "Cargo.toml").get("lints")
            != {"workspace": True}
        ]
        self.assertEqual(missing, [], "add `[lints] workspace = true`")

    def test_the_workspace_lints_hold_the_clippy_rules_of_ci(self):
        lints = manifest(ROOT / "Cargo.toml")["workspace"]["lints"]
        self.assertEqual(lints["rust"].get("warnings"), "deny")
        rules = {
            "pedantic": ("warn", -1),
            "print_stdout": ("deny", 0),
            "print_stderr": ("deny", 0),
            "dbg_macro": ("deny", 0),
        }
        # A group needs a lower priority than the single lints of the table,
        # or clippy fails on lint_groups_priority.
        found = {name: lints["clippy"].get(name) for name in rules}
        found = {
            name: (
                (value["level"], value.get("priority", 0))
                if isinstance(value, dict)
                else (value, 0)
            )
            for name, value in found.items()
        }
        self.assertEqual(found, rules)

    def test_clippy_command_lines_rely_on_the_workspace_lints(self):
        lines = [
            line.strip().rstrip("; \\")
            for text in (CI, MAKEFILE)
            for line in text.splitlines()
            if "cargo clippy" in line and not line.lstrip().startswith("#")
        ]
        self.assertTrue(lines)
        for line in lines:
            with self.subTest(line=line):
                self.assertNotIn("clippy::", line)
                self.assertTrue(line.endswith("-- -D warnings"), line)


if __name__ == "__main__":
    unittest.main()

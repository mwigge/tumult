#!/usr/bin/env python3
"""Tests for scripts/check-pr-body.py and the workflow that runs it."""

import importlib.util
import os
import re
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = Path(__file__).with_name("check-pr-body.py")
WORKFLOW = ROOT / ".github/workflows/pr-body.yml"
_spec = importlib.util.spec_from_file_location("check_pr_body", SCRIPT)
body_check = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(body_check)

FULL = """## Why

The docs are long.

## What changed

- A prose check.

## Scope

Scripts and CI only.

## Tradeoffs

Only added lines are read.

## Blast Radius

A pull request with a dash fails.

## Verification

The script tests pass.
"""

SHORT = """## Why

A flake.

## Verification

Ran it 200 times.
"""


def problems(body):
    return body_check.problems(body)


class Body(unittest.TestCase):
    def test_a_body_with_the_six_headings_in_order_passes(self):
        self.assertEqual(problems(FULL), [])

    def test_empty_sections_other_than_why_and_verification_may_be_dropped(self):
        self.assertEqual(problems(SHORT), [])

    def test_a_body_without_headings_fails_on_why_and_verification(self):
        found = problems("Bumps a crate.\n")
        self.assertEqual(len(found), 2, found)
        self.assertIn("Why", found[0])
        self.assertIn("Verification", found[1])

    def test_an_empty_body_fails(self):
        self.assertNotEqual(problems(""), [])

    def test_why_or_verification_without_text_fails(self):
        for body in (
            "## Why\n\n## Verification\n\nRan it.\n",
            "## Why\n\nA flake.\n\n## Verification\n",
        ):
            with self.subTest(body=body):
                self.assertEqual(len(problems(body)), 1, problems(body))

    def test_headings_out_of_order_fail(self):
        body = FULL.replace("## Scope", "## Tmp").replace("## Tradeoffs", "## Scope")
        found = problems(body.replace("## Tmp", "## Tradeoffs"))
        self.assertEqual(len(found), 1, found)
        self.assertIn("order", found[0])

    def test_a_heading_that_is_not_one_of_the_six_fails(self):
        for heading in ("## Notes", "## What Changed", "## Blast radius"):
            with self.subTest(heading=heading):
                found = problems(SHORT + f"\n{heading}\n\nText.\n")
                self.assertEqual(len(found), 1, found)
                self.assertIn(heading[3:], found[0])

    def test_a_repeated_heading_fails(self):
        self.assertEqual(len(problems(SHORT + "\n## Verification\n\nAgain.\n")), 1)

    def test_a_heading_in_a_code_block_or_of_another_level_is_not_a_section(self):
        body = SHORT + "\n```text\n## Notes\n```\n\n### Runs\n\n# Title\n"
        self.assertEqual(problems(body), [])

    def test_a_body_of_more_than_40_lines_fails(self):
        filler = "".join(f"line {n}\n" for n in range(34))
        at_limit = SHORT.replace("Ran it 200 times.\n", filler)
        self.assertEqual(len(at_limit.splitlines()), 40)
        self.assertEqual(problems(at_limit), [])
        found = problems(at_limit + "one more\n")
        self.assertEqual(len(found), 1, found)
        self.assertIn("41 lines", found[0])

    def test_windows_line_ends_are_read_like_unix_ones(self):
        self.assertEqual(problems(FULL.replace("\n", "\r\n")), [])


class Script(unittest.TestCase):
    def run_script(self, body, author="someone"):
        env = {**os.environ, "PR_BODY": body, "PR_AUTHOR": author}
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT)],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_the_body_comes_from_the_environment(self):
        passing = self.run_script(FULL)
        self.assertEqual(passing.returncode, 0, passing.stdout)
        failing = self.run_script("$(exit 3) `exit 4`\n## Notes\n")
        self.assertEqual(failing.returncode, 1, failing.stdout)
        self.assertIn("Notes", failing.stdout)

    def test_a_pull_request_by_dependabot_is_skipped(self):
        result = self.run_script("Bumps tokio from 1.0 to 1.1.\n", "dependabot[bot]")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("skipped", result.stdout)

    def test_a_missing_body_variable_is_an_error(self):
        env = {key: value for key, value in os.environ.items() if key != "PR_BODY"}
        result = subprocess.run(
            [sys.executable, "-B", str(SCRIPT)],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("PR_BODY", result.stderr)


class Workflow(unittest.TestCase):
    def setUp(self):
        self.text = WORKFLOW.read_text(encoding="utf-8")

    def test_it_runs_when_a_pull_request_opens_changes_or_is_edited(self):
        self.assertRegex(
            self.text,
            r"\n  pull_request:\n    types: \[opened, edited, synchronize, reopened\]\n",
        )

    def test_the_body_reaches_the_script_through_the_environment_only(self):
        expressions = re.findall(r"^(.*)\$\{\{([^}]*)\}\}", self.text, re.M)
        body = [line for line, expr in expressions if "pull_request.body" in expr]
        self.assertEqual(body, ["          PR_BODY: "])
        self.assertIn("run: python3 scripts/check-pr-body.py\n", self.text)
        for block in re.findall(r"run: \|\n((?:          .*\n)+)", self.text):
            self.assertNotIn("${{", block)

    def test_every_action_is_pinned_by_its_commit(self):
        uses = re.findall(r"uses: (\S+)", self.text)
        self.assertTrue(uses)
        for action in uses:
            self.assertRegex(action, r"@[0-9a-f]{40}$")


if __name__ == "__main__":
    unittest.main()

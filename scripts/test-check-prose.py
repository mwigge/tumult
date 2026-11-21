#!/usr/bin/env python3
"""Tests for scripts/check-prose.py."""

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-prose.py")
_spec = importlib.util.spec_from_file_location("check_prose", SCRIPT)
prose = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(prose)

EM = "\u2014"
EN = "\u2013"

# One added line, line 3 of the new file.
DASH_DIFF = f"""diff --git a/docs/guide.md b/docs/guide.md
index 1111111..2222222 100644
--- a/docs/guide.md
+++ b/docs/guide.md
@@ -2,0 +3 @@ # Guide
+The guard halts the run {EM} and says why.
"""

EN_DASH_DIFF = f"""diff --git a/README.md b/README.md
index 1111111..2222222 100644
--- a/README.md
+++ b/README.md
@@ -7 +7 @@
-Runs one to two minutes.
+Runs 1{EN}2 minutes.
"""

# Two added headings, lines 1 and 5 of a new file.
HEADING_DIFF = """diff --git a/docs/new.md b/docs/new.md
new file mode 100644
index 0000000..2222222
--- /dev/null
+++ b/docs/new.md
@@ -0,0 +1 @@
+# Blast Radius Of A Change
@@ -0,0 +5 @@
+## How the guard halts a run
"""


def offences(diff, files):
    return [
        (path, line, rule)
        for path, line, rule, _message in prose.check(diff, files.__getitem__)
    ]


def added(path, lines, start=1):
    """A diff that adds `lines` to `path` from line `start` on."""
    body = "".join(f"+{line}\n" for line in lines)
    return (
        f"diff --git a/{path} b/{path}\n"
        f"--- a/{path}\n+++ b/{path}\n"
        f"@@ -0,0 +{start},{len(lines)} @@\n{body}"
    )


def headings(*texts):
    """The Title Case offences of headings added to one markdown file."""
    lines = [f"## {text}" for text in texts]
    found = offences(added("docs/a.md", lines), {"docs/a.md": "\n".join(lines)})
    return [lines[line - 1][3:] for _path, line, _rule in found]


class Dashes(unittest.TestCase):
    def test_an_added_line_with_an_em_dash_fails_with_its_file_and_line(self):
        text = f"# Guide\n\nThe guard halts the run {EM} and says why.\n"
        self.assertEqual(
            offences(DASH_DIFF, {"docs/guide.md": text}), [("docs/guide.md", 3, "dash")]
        )

    def test_an_added_line_with_an_en_dash_fails(self):
        text = "\n" * 6 + f"Runs 1{EN}2 minutes.\n"
        self.assertEqual(
            offences(EN_DASH_DIFF, {"README.md": text}), [("README.md", 7, "dash")]
        )

    def test_a_hyphen_and_a_dash_in_inline_code_pass(self):
        lines = ["Use `--dry-run` - it is safe.", f"The table shows `{EM}` for none."]
        diff = added("docs/a.md", lines)
        self.assertEqual(offences(diff, {"docs/a.md": "\n".join(lines)}), [])

    def test_a_dash_on_a_line_the_diff_did_not_add_passes(self):
        diff = (
            "diff --git a/docs/a.md b/docs/a.md\n--- a/docs/a.md\n+++ b/docs/a.md\n"
            f"@@ -1 +1 @@\n-Old {EM} line.\n+New line.\n"
        )
        text = f"New line.\nAn old line {EM} that stays.\n"
        self.assertEqual(offences(diff, {"docs/a.md": text}), [])

    def test_an_added_line_that_starts_with_plus_signs_is_read_as_a_line(self):
        # An added line that starts with "++" looks like a file header.
        lines = [f"++ counts up {EM} by one."]
        diff = added("docs/a.md", lines)
        self.assertEqual(
            offences(diff, {"docs/a.md": lines[0]}), [("docs/a.md", 1, "dash")]
        )

    def test_html_under_docs_is_read_for_dashes_and_not_for_headings(self):
        lines = [f"<p>Fast {EM} and safe.</p>", "# Not A Markdown Heading"]
        diff = added("docs/index.html", lines)
        self.assertEqual(
            offences(diff, {"docs/index.html": "\n".join(lines)}),
            [("docs/index.html", 1, "dash")],
        )

    def test_files_other_than_markdown_and_html_under_docs_are_not_read(self):
        lines = [f"// a comment {EM} in code", "# Title Case Comment"]
        diff = "".join(
            added(path, lines)
            for path in ("tumult-cli/src/main.rs", "web/src/app.html", "docs/a.txt")
        )
        self.assertEqual(offences(diff, {}), [])


class Headings(unittest.TestCase):
    def test_a_title_case_heading_fails_and_a_sentence_case_one_passes(self):
        text = "# Blast Radius Of A Change\n\n\n\n## How the guard halts a run\n"
        self.assertEqual(
            offences(HEADING_DIFF, {"docs/new.md": text}),
            [("docs/new.md", 1, "title-case")],
        )

    def test_the_message_names_the_words_in_capitals(self):
        text = "# Blast Radius Of A Change\n\n\n\n## How the guard halts a run\n"
        found = prose.check(HEADING_DIFF, {"docs/new.md": text}.get)
        self.assertIn("Radius, Of, Change", found[0][3])

    def test_names_acronyms_and_code_in_a_heading_pass(self):
        self.assertEqual(
            headings(
                "Run Tumult with DuckDB, the CLI and the API",
                "Send OpenTelemetry traces to GitHub",
                "Krönika and Kronika on Kubernetes and Docker",
                "Draw MCP results with ECharts",
                "Map DORA articles to `RunRecord` rows",
                "What the Star Map shows",
                "Read [the Tumult guide](guide.md) first",
                "Plan B for a run on macOS",
                "Grafana with Tempo, Mimir and Loki in July",
                "Deploy with Docker Compose",
                "The Tumult Chaos Lab",
                "Edit Cargo.toml by hand",
            ),
            [],
        )

    def test_a_capital_before_a_number_is_a_label_and_passes(self):
        self.assertEqual(headings("Phase 2 and Article 28 of DORA", "See Layer 3"), [])

    def test_a_capital_after_a_colon_or_a_number_passes(self):
        self.assertEqual(headings("Step 1: Install the binary", "2. Run it"), [])

    def test_capitals_after_a_hyphen_a_slash_or_markup_fail(self):
        self.assertEqual(
            headings(
                "Read-Only mode",
                "Input/Output limits",
                "The **Guard** halts",
                "Lake Schema {#lake}",
            ),
            [
                "Read-Only mode",
                "Input/Output limits",
                "The **Guard** halts",
                "Lake Schema {#lake}",
            ],
        )

    def test_lines_in_a_fenced_code_block_are_skipped(self):
        lines = [
            "```sh",
            "# Start The Daemon",
            f"tumultd {EM} serve",
            "```",
            "~~~",
            "## Inside A Tilde Fence",
            "~~~",
            "## After The Fence",
        ]
        diff = added("docs/a.md", lines)
        self.assertEqual(
            offences(diff, {"docs/a.md": "\n".join(lines)}),
            [("docs/a.md", 8, "title-case")],
        )

    def test_an_added_line_in_a_fence_that_opened_above_the_diff_is_skipped(self):
        text = "```text\nline one\n# Shell Comment In Code\n```\n"
        diff = added("docs/a.md", ["# Shell Comment In Code"], start=3)
        self.assertEqual(offences(diff, {"docs/a.md": text}), [])


class Script(unittest.TestCase):
    """The script against a real repository in a temp dir."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name) / "repo"
        self.root.mkdir()
        # No repository, hook, signing key or prefix setting of the host
        # reaches git.
        empty = Path(self.tmp.name) / "gitconfig"
        empty.write_text("")
        self.env = {
            **{k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
            "HOME": self.tmp.name,
            "GIT_CONFIG_GLOBAL": str(empty),
            "GIT_CONFIG_NOSYSTEM": "1",
        }
        self.git("init", "-q", "-b", "main")
        (self.root / "docs").mkdir()
        self.write("docs/old.md", f"# Old guide\n\nAn old line {EM} that stays.\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "base")
        self.git("switch", "-q", "-c", "topic")

    def tearDown(self):
        self.tmp.cleanup()

    def git(self, *args):
        subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
            cwd=self.root,
            env=self.env,
            check=True,
            capture_output=True,
        )

    def write(self, rel, text):
        (self.root / rel).write_text(text, encoding="utf-8")

    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), *args],
            cwd=self.root,
            env=self.env,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_committed_and_uncommitted_lines_since_the_base_are_read(self):
        self.write("docs/new.md", "# New Guide Page\n\nPlain words.\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "topic")
        self.write(
            "docs/old.md", f"# Old guide\n\nAn old line {EM} that stays.\nA{EN}b\n"
        )
        result = self.run_script("main")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(
            [line.split(": ", 2)[:2] for line in lines[:-1]],
            [["docs/new.md:1", "title-case"], ["docs/old.md:4", "dash"]],
        )
        self.assertIn("2 prose failure(s)", lines[-1])

    def test_a_moved_file_is_read_only_on_its_changed_lines(self):
        self.git("mv", "docs/old.md", "docs/moved.md")
        self.write(
            "docs/moved.md", f"# Old guide\n\nAn old line {EM} that stays.\nMore.\n"
        )
        self.git("commit", "-q", "-am", "move")
        result = self.run_script("main")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_clean_diff_passes(self):
        self.write("docs/old.md", f"# Old guide\n\nAn old line {EM} that stays.\nOk.\n")
        result = self.run_script("main")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Prose check passed", result.stdout)

    def test_an_unknown_base_fails_with_the_git_error(self):
        result = self.run_script("no-such-ref")
        self.assertEqual(result.returncode, 2)
        self.assertIn("no-such-ref", result.stderr)


if __name__ == "__main__":
    unittest.main()

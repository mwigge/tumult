#!/usr/bin/env python3
"""Tests for the anchor and repository path checks of scripts/check-docs.py.
Each test writes a few pages in a git repository in a temp dir."""

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-docs.py")
_spec = importlib.util.spec_from_file_location("check_docs", SCRIPT)
check_docs = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check_docs)


class RepoTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)

    def write(self, path, text):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text, encoding="utf-8")


class Anchors(RepoTest):
    def errors(self):
        errors = []
        check_docs.check_anchors(errors, self.root)
        return errors

    def test_a_link_to_a_heading_of_the_page_passes(self):
        self.write(
            "docs/cli.md",
            "# CLI\n\n- [Run](#tumult-run)\n- [Env](#environment-variables)\n\n"
            "## tumult run\n\n## Environment variables\n",
        )
        self.assertEqual(self.errors(), [])

    def test_a_link_to_no_heading_of_the_page_fails(self):
        self.write("docs/cli.md", "# CLI\n\n[Run](#tumult-runs)\n\n## tumult run\n")
        self.assertEqual(
            self.errors(), ["docs/cli.md: link to a missing heading: #tumult-runs"]
        )

    def test_headings_are_read_the_way_github_and_the_site_read_them(self):
        self.write(
            "docs/standard.md",
            "[a](#tumult-run-flags) [b](#what-302-changed) [c](#step-1)"
            " [d](#a-b_c) [e](#the-store) [f](#shell)\n\n"
            "## `tumult run` flags\n\n## What 3.0.2 changed?\n\n"
            "## Step\n\n## Step\n\n## A b_c ##\n\n## The [store](store.md)\n\n"
            '<a id="shell"></a>\n',
        )
        self.assertEqual(self.errors(), [])

    def test_code_blocks_hold_no_links_and_no_headings(self):
        self.write(
            "README.md",
            "```markdown\n[Run](#nowhere)\n## Inside\n```\n\n"
            "[Inside](#inside) and `[Text](#code-span)`\n",
        )
        self.assertEqual(
            self.errors(), ["README.md: link to a missing heading: #inside"]
        )

    def test_a_link_to_a_heading_of_another_page_is_checked_on_that_page(self):
        self.write("docs/guides/run.md", "# Run\n\n## Flags\n")
        self.write(
            "docs/index.md",
            "[Flags](guides/run.md#flags) [Gone](guides/run.md#options)\n",
        )
        self.assertEqual(
            self.errors(),
            ["docs/index.md: link to a missing heading: guides/run.md#options"],
        )


class RepoPaths(RepoTest):
    def errors(self, generated=None):
        errors = []
        check_docs.check_repo_paths(errors, self.root, generated or {})
        return errors

    def test_a_backticked_path_that_exists_passes(self):
        self.write("tumult-core/src/runner/activity.rs", "")
        self.write(
            "docs/security.md",
            "See `tumult-core/src/runner/activity.rs:427`, the folder "
            "`tumult-core/src/runner/` and `tumult-core/src`.\n",
        )
        self.assertEqual(self.errors(), [])

    def test_a_backticked_path_that_does_not_exist_fails(self):
        self.write("tumult-core/src/runner/activity.rs", "")
        self.write("docs/security.md", "# A\n\nSee `tumult-core/src/runner.rs:563`.\n")
        self.assertEqual(
            self.errors(),
            [
                "docs/security.md:3: no such repository path: tumult-core/src/runner.rs:563"
            ],
        )

    def test_a_token_that_is_not_a_repository_path_is_not_checked(self):
        self.write("docs/x.md", "")
        self.write(
            "docs/guide.md",
            "`docs/*.md` `docs/{a,b}.md` `docs/<page>.md` `~/.tumult/lake.duckdb`"
            " `https://tumult.rs/docs/x` `api/runs/{id}` `GET /api/runs` `k8s/pod`"
            " `README.md`\n",
        )
        self.assertEqual(self.errors(), [])

    def test_a_path_git_ignores_fails_unless_it_is_a_generated_path(self):
        self.write(".gitignore", "web/build/\n")
        self.write("web/build/index.html", "")
        self.write("web/README.md", "Build `web/build/` first.\n")
        self.assertEqual(
            self.errors(), ["web/README.md:1: no such repository path: web/build/"]
        )
        self.assertEqual(self.errors({"web/build": "the UI build"}), [])

    def test_a_generated_path_that_no_page_names_fails(self):
        self.write("README.md", "Nothing here.\n")
        self.assertEqual(
            self.errors({"web/build": "the UI build"}),
            [
                "scripts/check-docs.py: GENERATED_PATHS lists web/build, no page names it"
            ],
        )

    def test_a_page_of_a_sub_project_may_name_its_own_paths(self):
        self.write("scripts/check-docs.py", "")
        self.write("chaos-lab/scripts/install_tumult.py", "")
        self.write("chaos-lab/docs/reference.md", "Edit `scripts/install_tumult.py`.\n")
        self.assertEqual(self.errors(), [])

    def test_dated_records_are_not_checked(self):
        self.write("tumult-core/src/lib.rs", "")
        for page in (
            "CHANGELOG.md",
            "chaos-lab/CHANGELOG.md",
            "docs/releases/1.0.md",
            "docs/blog/01-start.md",
        ):
            self.write(page, "Moved `tumult-core/src/runner.rs`.\n")
        self.assertEqual(self.errors(), [])


if __name__ == "__main__":
    unittest.main()

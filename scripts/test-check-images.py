#!/usr/bin/env python3
"""Tests for scripts/check-images.py. Each test builds a small repository in a
temp dir, except the last, which checks this repository."""

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-images.py")
ROOT = SCRIPT.resolve().parents[1]
_spec = importlib.util.spec_from_file_location("check_images", SCRIPT)
check_images = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check_images)

PNG = b"\x89PNG\r\n\x1a\n"


class RepoTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.allow = self.root / "allow.txt"
        self.allow.write_text("", encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)

    def write(self, path, text):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text, encoding="utf-8")

    def image(self, path):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_bytes(PNG)

    def track(self):
        subprocess.run(["git", "-C", str(self.root), "add", "-A"], check=True)

    def failures(self):
        return [
            (path, rule) for path, rule, _ in check_images.check(self.root, self.allow)
        ]


class Missing(RepoTest):
    def test_a_page_that_names_a_missing_image_fails(self):
        self.write("docs/guide.md", "![Login](../assets/login.png)\n")
        self.assertEqual(self.failures(), [("docs/guide.md", "missing")])

    def test_an_image_deleted_from_the_tree_counts_as_missing(self):
        self.write("docs/guide.md", "![Login](images/login.png)\n")
        self.image("docs/images/login.png")
        self.track()
        (self.root / "docs/images/login.png").unlink()
        self.assertEqual(self.failures(), [("docs/guide.md", "missing")])

    def test_a_root_path_in_a_docs_page_is_read_from_the_docs_folder(self):
        self.write("docs/index.html", '<img src="/images/a.png" alt="A">\n')
        self.image("docs/images/a.png")
        self.assertEqual(self.failures(), [])

    def test_a_root_path_outside_the_docs_folder_is_read_from_the_root(self):
        self.write("README.md", "![A](/docs/images/a.png)\n")
        self.image("docs/images/a.png")
        self.assertEqual(self.failures(), [])

    def test_external_and_template_images_are_not_checked(self):
        self.write(
            "README.md",
            "![CI](https://example.com/badge.svg)\n"
            '<img src="{{ site.logo }}">\n'
            "![Inline](data:image/png;base64,AAAA)\n",
        )
        self.assertEqual(self.failures(), [])

    def test_an_image_named_only_in_prose_or_code_is_not_a_reference(self):
        self.write("CHANGELOG.md", "Replaced `login.png` and docs/images/old.png.\n")
        self.assertEqual(self.failures(), [])


class Unnamed(RepoTest):
    def test_an_image_that_no_page_names_fails(self):
        self.image("docs/assets/orphan.png")
        self.assertEqual(self.failures(), [("docs/assets/orphan.png", "unnamed")])

    def test_an_image_outside_the_two_image_folders_is_not_checked(self):
        self.image("web/static/icon.png")
        self.assertEqual(self.failures(), [])

    def test_markdown_html_front_matter_and_site_config_name_images(self):
        self.write(
            "docs/guide.md",
            "---\nimage: /images/card.png\n---\n\n"
            '![Run](images/run.png "The run")\n'
            "[the diagram](images/flow.svg)\n"
            "[shot]: images/ref.png\n",
        )
        self.write(
            "docs/index.html",
            '<meta property="og:image" content="/images/og.png">\n'
            "<img alt='Map' src='/images/map.png'>\n",
        )
        self.write("docs/_config.yml", 'logo: "/images/logo.png"\n')
        for name in ("card", "run", "ref", "og", "map", "logo"):
            self.image(f"docs/images/{name}.png")
        self.image("docs/images/flow.svg")
        self.assertEqual(self.failures(), [])

    def test_an_image_in_the_allow_file_with_a_reason_passes(self):
        self.image("docs/images/src/flow.svg")
        self.allow.write_text(
            "# Sources.\ndocs/images/src/flow.svg  # source of a diagram\n",
            encoding="utf-8",
        )
        self.assertEqual(self.failures(), [])


class Allow(RepoTest):
    def test_an_allow_line_without_a_reason_fails(self):
        self.image("docs/images/src/flow.svg")
        self.allow.write_text("docs/images/src/flow.svg\n", encoding="utf-8")
        self.assertEqual(
            self.failures(),
            [
                ("allow.txt", "bad-allow"),
                ("docs/images/src/flow.svg", "unnamed"),
            ],
        )

    def test_an_allow_entry_for_a_gone_file_fails(self):
        self.allow.write_text("docs/images/gone.png  # kept\n", encoding="utf-8")
        self.assertEqual(self.failures(), [("docs/images/gone.png", "stale-allow")])

    def test_an_allow_entry_for_a_named_image_fails(self):
        self.write("docs/guide.md", "![A](images/a.png)\n")
        self.image("docs/images/a.png")
        self.allow.write_text("docs/images/a.png  # kept\n", encoding="utf-8")
        self.assertEqual(self.failures(), [("docs/images/a.png", "stale-allow")])


class Command(RepoTest):
    def test_the_command_names_each_failure_and_exits_one(self):
        self.write("docs/guide.md", "![Gone](images/gone.png)\n")
        self.image("docs/assets/orphan.png")
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--root",
                str(self.root),
                "--allow",
                str(self.allow),
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("docs/guide.md: missing: images/gone.png", result.stdout)
        self.assertIn("docs/assets/orphan.png: unnamed:", result.stdout)


class Repository(unittest.TestCase):
    maxDiff = None

    def test_every_named_image_exists_and_every_image_is_named(self):
        found = [
            f"{path}: {rule}: {detail}"
            for path, rule, detail in check_images.check(ROOT, check_images.ALLOW)
        ]
        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()

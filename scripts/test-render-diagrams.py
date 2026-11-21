#!/usr/bin/env python3
"""Tests for scripts/render-diagrams.mjs. Each test copies diagram sources into
a temp dir and runs the script on that dir with Node and the pinned tools of
scripts/diagrams (run `npm ci` and `npx playwright install chromium` there
first). archify checks the source paths of a diagram in this git checkout, so
the revisions the sources pin must be in its history."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "render-diagrams.mjs"
DIAGRAMS = ROOT / "docs" / "images" / "3.0" / "diagrams"
NAME = "c4-1-context"


def browsers_dir():
    """Where Playwright keeps its browsers, so HOME can point at the temp dir."""
    if os.environ.get("PLAYWRIGHT_BROWSERS_PATH"):
        return os.environ["PLAYWRIGHT_BROWSERS_PATH"]
    cache = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return str(Path(cache) / "ms-playwright")


class Render(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.dir = self.root / "diagrams"
        self.dir.mkdir()

    def copy(self, *files):
        for name in files:
            shutil.copyfile(DIAGRAMS / name, self.dir / name)

    def render(self, *args):
        env = {
            **os.environ,
            "PLAYWRIGHT_BROWSERS_PATH": browsers_dir(),
            "HOME": str(self.root),
            "TMPDIR": str(self.root),
        }
        return subprocess.run(
            ["node", str(SCRIPT), "--dir", str(self.dir), *args],
            capture_output=True,
            text=True,
            env=env,
            timeout=300,
            check=False,
        )

    def test_a_source_renders_to_the_committed_svg_byte_for_byte(self):
        self.copy(f"{NAME}.json")
        result = self.render()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(
            (self.dir / f"{NAME}.svg").read_bytes(),
            (DIAGRAMS / f"{NAME}.svg").read_bytes(),
        )
        self.assertEqual(
            sorted(p.name for p in self.dir.iterdir()),
            [
                f"{NAME}.json",
                f"{NAME}.svg",
            ],
        )

    def test_the_check_passes_when_the_picture_is_what_its_source_renders(self):
        self.copy(f"{NAME}.json", f"{NAME}.svg")
        result = self.render("--check")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("1 diagram(s) match their sources", result.stdout)

    def test_the_check_fails_on_a_source_changed_without_a_new_picture(self):
        self.copy(f"{NAME}.svg")
        source = (DIAGRAMS / f"{NAME}.json").read_text("utf-8")
        changed = source.replace('"label": "Operator"', '"label": "Operators"')
        self.assertNotEqual(changed, source)
        (self.dir / f"{NAME}.json").write_text(changed, "utf-8")
        result = self.render("--check")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"{NAME}.svg: differs:", result.stdout)
        self.assertEqual(
            (self.dir / f"{NAME}.svg").read_bytes(),
            (DIAGRAMS / f"{NAME}.svg").read_bytes(),
            "the check must not write into the diagram dir",
        )

    def test_the_check_fails_on_a_source_with_no_picture(self):
        self.copy(f"{NAME}.json")
        result = self.render("--check")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"{NAME}.svg: missing:", result.stdout)
        self.assertFalse((self.dir / f"{NAME}.svg").exists())

    def test_the_check_fails_on_a_picture_that_no_source_draws(self):
        self.copy(f"{NAME}.json", f"{NAME}.svg")
        (self.dir / "old-flow.svg").write_text("<svg/>", "utf-8")
        (self.dir / f"{NAME}.png").write_bytes(b"\x89PNG\r\n")
        result = self.render("--check")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("old-flow.svg: no-source:", result.stdout)
        self.assertIn(f"{NAME}.png: no-source:", result.stdout)
        self.assertNotIn(f"{NAME}.svg:", result.stdout)

    def test_a_source_that_archify_refuses_fails_with_its_reason(self):
        source = (DIAGRAMS / "flow-evidence-loop.json").read_text("utf-8")
        old = source.replace(
            "b8304ddd84ea7522550cf84687aa0c7c784438e7",
            "8c24680b94cbab9d5b2011d99b53623618608a22",
        )
        self.assertNotEqual(old, source)
        (self.dir / "flow-evidence-loop.json").write_text(old, "utf-8")
        result = self.render()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(
            "flow-evidence-loop.json: render-failed: "
            "[repository-evidence/file-missing]",
            result.stdout,
        )
        self.assertFalse((self.dir / "flow-evidence-loop.svg").exists())

    def test_an_unknown_name_fails_before_any_render(self):
        self.copy(f"{NAME}.json")
        result = self.render("no-such-diagram")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("no-such-diagram: no source", result.stderr)
        self.assertFalse((self.dir / f"{NAME}.svg").exists())


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Tests for scripts/site-shots.mjs. Each test builds a small site in a temp dir
and runs the script with Node and the Playwright of web/node_modules (run
`npm ci` and `npx playwright install chromium` in web first)."""

import json
import os
import struct
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("site-shots.mjs")
PAGES = {
    "index.html": "Landing",
    "get-started/index.html": "Get started",
    "guides/index.html": "Docs home",
    "guides/observability-setup.html": "A guide",
    "guides/platform-walkthrough.html": "The walkthrough",
    "reference/kronika-pages.html": "A reference page",
    "releases/index.html": "Releases",
}
WIDE = '<div style="width: 600px; height: 20px; background: #1f6fb2"></div>'


def page(title, body=""):
    return (
        '<!DOCTYPE html><html><head><meta name="viewport" '
        'content="width=device-width, initial-scale=1">'
        f"<title>{title}</title><style>body{{margin:0;font:16px sans-serif}}</style>"
        f"</head><body><h1>{title}</h1>{body}</body></html>"
    )


def png_size(path):
    """Width and height from the IHDR chunk of a PNG file."""
    data = path.read_bytes()[:24]
    return struct.unpack(">II", data[16:24])


def browsers_dir():
    """Where Playwright keeps its browsers, so HOME can point at the temp dir."""
    if os.environ.get("PLAYWRIGHT_BROWSERS_PATH"):
        return os.environ["PLAYWRIGHT_BROWSERS_PATH"]
    cache = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return str(Path(cache) / "ms-playwright")


class Shots(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.site = self.root / "_site"
        self.out = self.root / "shots"
        self.known = self.root / "known.txt"
        self.known.write_text("# known\n", encoding="utf-8")

    def build(self, wide=(), leave_out=()):
        for name, title in PAGES.items():
            if name in leave_out:
                continue
            path = self.site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(page(title, WIDE if name in wide else ""), "utf-8")

    def shoot(self):
        env = {
            **os.environ,
            "PLAYWRIGHT_BROWSERS_PATH": browsers_dir(),
            "HOME": str(self.root),
            "TMPDIR": str(self.root),
        }
        return subprocess.run(
            ["node", str(SCRIPT), str(self.site), str(self.out)]
            + ["--known", str(self.known)],
            capture_output=True,
            text=True,
            env=env,
            timeout=300,
            check=False,
        )

    def test_a_page_wider_than_a_phone_fails_with_its_page_and_width(self):
        self.build(wide=["guides/platform-walkthrough.html"])
        result = self.shoot()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(
            "guides/platform-walkthrough.html: overflow-390: 600 px wide",
            result.stdout,
        )
        self.assertNotIn("overflow-1440", result.stdout)

    def test_every_page_is_shot_at_both_widths_full_and_first_screen(self):
        self.build()
        result = self.shoot()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        names = sorted(path.name for path in self.out.glob("*.png"))
        self.assertEqual(len(names), 28, names)
        self.assertEqual(png_size(self.out / "releases-390-fold.png"), (390, 844))
        self.assertEqual(png_size(self.out / "landing-1440-fold.png"), (1440, 900))
        self.assertEqual(png_size(self.out / "landing-390-full.png")[0], 390)
        summary = json.loads((self.out / "shots.json").read_text("utf-8"))
        self.assertEqual(len(summary), 14)

    def test_a_known_overflow_passes_and_is_printed(self):
        self.build(wide=["index.html"])
        self.known.write_text("index.html::overflow-390  # install line\n", "utf-8")
        result = self.shoot()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("known: index.html: overflow-390: ", result.stdout)

    def test_a_known_overflow_that_is_gone_fails_as_stale(self):
        self.build()
        self.known.write_text("index.html::overflow-390  # install line\n", "utf-8")
        result = self.shoot()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("stale-known", result.stdout)

    def test_a_page_of_the_list_that_the_site_lacks_fails_before_any_shot(self):
        self.build(leave_out=["releases/index.html"])
        result = self.shoot()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("releases/index.html: not-found: ", result.stdout)
        self.assertEqual(list(self.out.glob("*.png")), [])


if __name__ == "__main__":
    unittest.main()

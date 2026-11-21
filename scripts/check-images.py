#!/usr/bin/env python3
"""Check that the pages and the image files of the repository agree.

Fails, with the file and the reason, on:
  missing      a page names an image file that is not there
  unnamed      an image file under docs/images or docs/assets that no page
               names and that the allow file does not list
  stale-allow  an allow entry whose file is gone, or that a page names
  bad-allow    an allow line that is not `path  # reason`

A page is a markdown or html file of the repository, or docs/_config.yml.
Files come from git: tracked files that are still there, and new files that
git does not ignore. A page names an image in a markdown image or link, a
markdown link definition, an html `src`, `href`, `content` or `poster`
attribute, or a yaml value of its front matter (all of docs/_config.yml).
A name in prose or in backticks is not a reference. Links to other sites,
`data:` URIs and Liquid templates are not checked.

A path that starts with `/` is read from the docs folder for a page under
docs/ (the root of the site), and from the repository root for other pages.

The allow file lists image files that no page names on purpose, one
`path  # reason` per line, with the path from the repository root.

Usage: scripts/check-images.py [--root DIR] [--allow FILE]
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
ALLOW = Path(__file__).with_name("images-allow.txt")
IMAGE_DIRS = ("docs/images/", "docs/assets/")
IMAGE_SUFFIXES = (".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".avif", ".ico")
PAGE_SUFFIXES = (".md", ".html")
SITE_CONFIG = "docs/_config.yml"

MD_LINK = re.compile(r"!?\[[^\]]*\]\(\s*<?([^)\s>]+)")
MD_DEFINITION = re.compile(r"^ {0,3}\[[^\]]+\]:\s*<?([^\s>]+)", re.M)
HTML_ATTR = re.compile(r"""\b(?:src|href|content|poster)\s*=\s*(["'])(.*?)\1""", re.S)
YAML_VALUE = re.compile(r"""^\s*[\w-]+:\s*(["']?)([^"'\s#]+)\1\s*(?:#.*)?$""", re.M)
FRONT_MATTER = re.compile(r"\A---\n(.*?)\n---\n", re.S)
ALLOW_LINE = re.compile(r"(\S+)\s+#\s*(\S.*)")


def repository_files(root: Path) -> list[str]:
    """Tracked files that are still there, and new files git does not ignore."""
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others"]
        + ["--exclude-standard"],
        capture_output=True,
        check=True,
    ).stdout.decode("utf-8")
    names = sorted({name for name in out.split("\0") if name})
    return [name for name in names if (root / name).is_file()]


def targets(page: str, text: str) -> list[str]:
    """Every link, image or yaml value of a page, before it is resolved."""
    found = MD_LINK.findall(text) + MD_DEFINITION.findall(text)
    found += [value for _, value in HTML_ATTR.findall(text)]
    yaml = text if page == SITE_CONFIG else ""
    front = FRONT_MATTER.match(text)
    if front:
        yaml = front[1]
    found += [value for _, value in YAML_VALUE.findall(yaml)]
    return found


def image_path(page: str, target: str) -> str | None:
    """The repository path an image target of a page points at, or None when
    the target is not a local image."""
    target = target.strip()
    if "{{" in target or "{%" in target or "://" in target:
        return None
    if target.startswith(("data:", "mailto:", "#", "//")):
        return None
    target = unquote(target.split("#", 1)[0].split("?", 1)[0])
    if not target.lower().endswith(IMAGE_SUFFIXES):
        return None
    if target.startswith("/"):
        base = "docs" if page.startswith("docs/") else ""
        return os.path.normpath(base + target).lstrip("/")
    return os.path.normpath(os.path.join(os.path.dirname(page), target))


def read_allow(allow: Path, failures: list) -> list[str]:
    """The paths of the allow file. A line with no reason is a failure."""
    if not allow.is_file():
        return []
    entries = []
    for line in allow.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        match = ALLOW_LINE.fullmatch(line)
        if not match:
            failures.append((allow.name, "bad-allow", line))
            continue
        entries.append(match[1])
    return entries


def check(root: Path, allow: Path) -> list[tuple[str, str, str]]:
    """Every failure as (file, rule, detail), sorted by file."""
    failures: list[tuple[str, str, str]] = []
    files = repository_files(root)
    present = set(files)
    named = set()
    for page in files:
        if not (page.endswith(PAGE_SUFFIXES) or page == SITE_CONFIG):
            continue
        text = (root / page).read_text(encoding="utf-8", errors="replace")
        for target in targets(page, text):
            path = image_path(page, target)
            if path is None:
                continue
            named.add(path)
            if path not in present:
                failures.append((page, "missing", target))
    allowed = read_allow(allow, failures)
    for path in allowed:
        if path not in present:
            failures.append((path, "stale-allow", "the file is gone"))
        elif path in named:
            failures.append((path, "stale-allow", "a page names it"))
    for path in files:
        if not path.startswith(IMAGE_DIRS) or not path.lower().endswith(IMAGE_SUFFIXES):
            continue
        if path not in named and path not in allowed:
            failures.append(
                (path, "unnamed", "no page names it, and it is not allowed")
            )
    return sorted(set(failures))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--allow", type=Path, default=ALLOW)
    args = parser.parse_args()
    failures = check(args.root, args.allow)
    for path, rule, detail in failures:
        sys.stdout.write(f"{path}: {rule}: {detail}\n")
    if failures:
        sys.stdout.write(f"{len(failures)} image failures\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

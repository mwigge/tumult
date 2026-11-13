#!/usr/bin/env python3
"""Fail on regulatory citation wordings that were found to be wrong."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEXT_SUFFIXES = {
    ".html",
    ".js",
    ".json",
    ".md",
    ".py",
    ".rs",
    ".sh",
    ".sql",
    ".svelte",
    ".toml",
    ".toon",
    ".ts",
    ".txt",
    ".yaml",
    ".yml",
}
FORBIDDEN = (
    (
        re.compile(r"Art\. 24\(7\)"),
        "DORA Art. 24 has six paragraphs; cite 24(6) for the yearly test "
        "or 11(6) for a continuity plan or tabletop exercise",
    ),
    (
        re.compile(r"Art\. 28\) retention|years \(Art\. 28\)"),
        "DORA Art. 28 is ICT third-party risk and sets no retention period",
    ),
    (
        re.compile(r"Article 26 — ICT testing"),
        "DORA Art. 26 is threat-led penetration testing; "
        "ICT testing is Art. 24 and Art. 25",
    ),
)


def tracked_text_files() -> list[Path]:
    listing = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    paths = (ROOT / name for name in listing.split("\0") if name)
    own = Path(__file__).resolve()
    return [
        path
        for path in paths
        if path.suffix in TEXT_SUFFIXES and path.is_file() and path != own
    ]


def check_file(path: Path) -> list[str]:
    errors = []
    lines = path.read_text(encoding="utf-8").splitlines()
    for number, line in enumerate(lines, start=1):
        for pattern, reason in FORBIDDEN:
            match = pattern.search(line)
            if match is None:
                continue
            where = f"{path.relative_to(ROOT)}:{number}"
            errors.append(f"{where}: {match.group(0)!r}: {reason}")
    return errors


def main() -> int:
    files = tracked_text_files()
    errors = [error for path in files for error in check_file(path)]
    if errors:
        print("Citation checks failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(f"Citation checks passed: {len(files)} files, {len(FORBIDDEN)} patterns.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

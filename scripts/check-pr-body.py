#!/usr/bin/env python3
"""Check the body of a pull request. See CONTRIBUTING.md.

The body has these `##` headings in this order: Why, What changed, Scope,
Tradeoffs, Blast Radius, Verification. A section with nothing to say may be
dropped, except Why and Verification, which must have text. The body has at
most 40 lines. Long evidence goes in a file or a comment that the body links.

The body comes from the environment variable PR_BODY, never from the text of
a shell command. A pull request by Dependabot (PR_AUTHOR) is skipped.

Usage: PR_BODY="$(cat body.md)" scripts/check-pr-body.py
"""

from __future__ import annotations

import os
import re
import sys

HEADINGS = (
    "Why",
    "What changed",
    "Scope",
    "Tradeoffs",
    "Blast Radius",
    "Verification",
)
REQUIRED = ("Why", "Verification")
MAX_LINES = 40
SKIPPED_AUTHORS = {"dependabot[bot]"}
SECTION = re.compile(r"^##[ \t]+(.*?)[ \t]*#*[ \t]*$")
FENCE = re.compile(r"^[ \t]*(`{3,}|~{3,})")


def sections(lines: list[str]) -> list[tuple[str, bool]]:
    """(heading, has text) for each `##` heading outside a code block."""
    found: list[tuple[str, bool]] = []
    fence = ""
    for line in lines:
        mark = FENCE.match(line)
        if mark and (not fence or mark[1].startswith(fence)):
            fence = "" if fence else mark[1]
        heading = None if fence or mark else SECTION.match(line)
        if heading:
            found.append((heading[1], False))
        elif found and line.strip():
            found[-1] = (found[-1][0], True)
    return found


def problems(body: str) -> list[str]:
    """What is wrong with a body."""
    lines = body.splitlines()
    found = sections(lines)
    names = [name for name, _text in found]
    out = [
        f"unknown heading `## {name}`; the headings are: {', '.join(HEADINGS)}"
        for name in names
        if name not in HEADINGS
    ]
    known = list(dict.fromkeys(name for name in names if name in HEADINGS))
    out += [f"`## {name}` comes twice" for name in known if names.count(name) > 1]
    if known != sorted(known, key=HEADINGS.index):
        out.append(f"headings out of order: {', '.join(known)}")
    for name in REQUIRED:
        if not any(heading == name and text for heading, text in found):
            out.append(f"`## {name}` with text is required")
    if len(lines) > MAX_LINES:
        out.append(f"{len(lines)} lines; the body has at most {MAX_LINES}")
    return out


def main() -> int:
    author = os.environ.get("PR_AUTHOR", "")
    if author in SKIPPED_AUTHORS:
        print(f"Pull request body check skipped for {author}.")
        return 0
    body = os.environ.get("PR_BODY")
    if body is None:
        print("check-pr-body: PR_BODY is not set.", file=sys.stderr)
        return 2
    found = problems(body)
    for problem in found:
        print(f"pull request body: {problem}")
    if found:
        print("See CONTRIBUTING.md.")
        return 1
    print("Pull request body check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

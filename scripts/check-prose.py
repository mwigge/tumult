#!/usr/bin/env python3
"""Prose check of the lines a branch adds. See CONTRIBUTING.md.

Reads the lines that `git diff` adds since the point where the work tree left
a base ref (default origin/main), in `*.md` files anywhere and in `*.html`
files under docs/. Fails on:
  dash        an em dash or an en dash
  title-case  a markdown heading in Title Case (sentence case is required)

Text in inline code and lines in fenced code blocks are not read. Names,
acronyms, labels and code keep their capitals: a word in ALLOWED or
ALLOWED_PHRASES, a word with a capital, a digit or a dot after its first
letter (DORA, DuckDB, NIS2, Cargo.toml), a single letter, and a word before a
number (Phase 2). The first word after a colon or the end of a sentence may
start with a capital.

Its limits: only ATX headings (`#`) are read, html is read for dashes only, and
a file that git does not track yet is not read.

Usage: scripts/check-prose.py [base ref]
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

DASHES = re.compile("[\u2013\u2014]")
INLINE_CODE = re.compile(r"(`+).+?\1")
FENCE = re.compile(r"^[ \t]*(`{3,}|~{3,})")
CLOSING_FENCE = re.compile(r"^[ \t]*(`{3,}|~{3,})[ \t]*$")
HEADING = re.compile(r"^ {0,3}#{1,6}(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$")
HUNK = re.compile(r"^@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
# Product names, tools and other proper nouns that keep their capital.
ALLOWED = {
    "Arrow",
    "Azure",
    "Copilot",
    "Docker",
    "Grafana",
    "Helm",
    "Jaeger",
    "Kafka",
    "Kronika",
    "Krönika",
    "Kubernetes",
    "Linux",
    "Loki",
    "Markdown",
    "Mermaid",
    "Mimir",
    "Ollama",
    "Parquet",
    "Playwright",
    "Prometheus",
    "Pumba",
    "Python",
    "Redis",
    "Rust",
    "Svelte",
    "Tempo",
    "Tumult",
    "Vitest",
    "Windows",
    *(
        "January February March April May June July August September October"
        " November December"
    ).split(),
}
ALLOWED_PHRASES = ("Star Map", "Game Day", "Chaos Lab", "Docker Compose")
CLAUSE_END = re.compile(r"[:.!?](?:\s|$)")
WORD_EDGES = "\"'“”‘’()[]{}*_,;!?.:"


def wanted(path: str) -> bool:
    """A markdown file anywhere, or an html file under docs/."""
    return path.endswith(".md") or (path.startswith("docs/") and path.endswith(".html"))


def added_lines(diff: str) -> dict[str, list[int]]:
    """The numbers (in the new file) of the lines a `git diff -U0` adds."""
    added: dict[str, list[int]] = {}
    path, line, old_left, new_left = None, 0, 0, 0
    for row in diff.splitlines():
        if old_left or new_left:
            if row.startswith("+"):
                added.setdefault(path, []).append(line)
                line, new_left = line + 1, new_left - 1
            elif row.startswith("-"):
                old_left -= 1
            elif row.startswith(" "):
                line, old_left, new_left = line + 1, old_left - 1, new_left - 1
            continue
        if row.startswith("+++ "):
            name = row[4:].rstrip("\t")
            path = name[2:] if name.startswith("b/") else None
        elif hunk := HUNK.match(row):
            old_left = int(hunk[1] or 1)
            line = int(hunk[2])
            new_left = int(hunk[3] or 1)
    return {path: lines for path, lines in added.items() if path and wanted(path)}


def fenced_lines(text: str) -> set[int]:
    """The numbers of the lines in fenced code blocks, fences included."""
    inside, opener, numbers = False, "", set()
    for number, row in enumerate(text.splitlines(), 1):
        fence = (CLOSING_FENCE if inside else FENCE).match(row)
        if not inside and fence:
            inside, opener = True, fence[1]
        elif inside and fence and fence[1][0] == opener[0]:
            inside = len(fence[1]) < len(opener)
        elif not inside:
            continue
        numbers.add(number)
    return numbers


def _allowed(word: str, after: str) -> bool:
    """A name, a single letter, a word with a capital, a digit or a dot after
    its first letter (DORA, APIs, DuckDB, NIS2, Cargo.toml), or a label: a
    word before a number (Phase 2)."""
    return (
        word in ALLOWED
        or len(word) == 1
        or any(char.isupper() or char.isdigit() or char == "." for char in word[1:])
        or after[:1].isdigit()
    )


def _word(token: str) -> str:
    return re.sub(r"['’]s$", "", token.strip(WORD_EDGES))


def title_case_words(heading: str) -> list[str]:
    """The words of a heading that make it Title Case."""
    text = INLINE_CODE.sub(" ", heading)
    text = re.sub(r"\]\([^)]*\)|<[^>]*>|\{[^}]*\}", " ", text)
    for phrase in ALLOWED_PHRASES:
        text = text.replace(phrase, " ")
    words = []
    for clause in CLAUSE_END.split(text):
        tokens = [_word(token) for token in re.split(r"[\s/-]+", clause)]
        tokens = [token for token in tokens if token]
        first = next((i for i, token in enumerate(tokens) if token[0].isalpha()), 0)
        for i, token in enumerate(tokens[first + 1 :], first + 1):
            after = tokens[i + 1] if i + 1 < len(tokens) else ""
            if token[0].isupper() and not _allowed(token, after):
                words.append(token)
    return words


def check_line(path: str, row: str) -> list[tuple[str, str]]:
    """The (rule, message) pairs of one line that is not code."""
    found = []
    if DASHES.search(INLINE_CODE.sub("", row)):
        found.append(("dash", "an em or en dash; use a comma, a colon or a period"))
    heading = HEADING.match(row) if path.endswith(".md") else None
    words = title_case_words(heading[1] or "") if heading else []
    if words:
        found.append(
            (
                "title-case",
                f"heading in Title Case ({', '.join(words)}); use sentence case",
            )
        )
    return found


def check(diff: str, read) -> list[tuple[str, int, str, str]]:
    """(path, line, rule, message) for each added line that breaks a rule.
    `read(path)` returns the text of the file the diff leads to."""
    offences = []
    for path, numbers in sorted(added_lines(diff).items()):
        text = read(path)
        rows = text.splitlines()
        code = fenced_lines(text)
        for number in numbers:
            if number in code or number > len(rows):
                continue
            for rule, message in check_line(path, rows[number - 1]):
                offences.append((path, number, rule, message))
    return offences


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-c", "core.quotePath=false", *args],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def diff_since(root: Path, base: str) -> str:
    """The work tree against the point where it left `base`, with moves
    found, so a moved file shows only the lines that changed."""
    fork = git(root, "merge-base", base, "HEAD").strip()
    return git(
        root,
        "diff",
        "-U0",
        "-M",
        "--no-color",
        "--no-ext-diff",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        fork,
        "--",
        "*.md",
        "docs/*.html",
    )


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("base", nargs="?", default="origin/main")
    args = parser.parse_args(argv)
    try:
        root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip())
        diff = diff_since(root, args.base)
    except subprocess.CalledProcessError as error:
        print(f"check-prose: git failed: {error.stderr.strip()}", file=sys.stderr)
        return 2
    offences = check(diff, lambda path: (root / path).read_text(encoding="utf-8"))
    for path, line, rule, message in offences:
        print(f"{path}:{line}: {rule}: {message}")
    if offences:
        print(
            f"{len(offences)} prose failure(s) in lines added since {args.base}. "
            "See CONTRIBUTING.md."
        )
        return 1
    print(f"Prose check passed: the lines added since {args.base}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

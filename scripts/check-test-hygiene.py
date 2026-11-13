#!/usr/bin/env python3
"""Cheap test-hygiene guard. See docs/guides/testing.md.

Fails on:
  no-assert      a Rust test whose body asserts nothing
  bare-ignore    `#[ignore]` without a reason string
  focused-test   `test.only` / `it.only` / `describe.only` / `fdescribe` / `fit`
  bare-skip      a Python skip decorator without a reason

Known offenders in crates that are being changed elsewhere are listed in
scripts/test-hygiene-allow.txt, one `path::name  # reason` per line. An entry
that no longer matches anything is reported so the list shrinks over time.

This is a text scan, not a parser: it cannot judge whether an assertion is
meaningful. Review against the policy does that.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALLOWLIST = Path(__file__).with_name("test-hygiene-allow.txt")
SKIP_DIRS = {".git", ".claude", "target", "node_modules", "dist", ".quality-venv"}
JS_SUFFIXES = {".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx"}

TEST_ATTR = re.compile(r"#\[\s*(?:\w+::)*test(?:\s*\([^\]]*\))?\s*\]")
ATTR = re.compile(r"\s*#\[[^\]]*\]")
FN = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)")
BARE_IGNORE = re.compile(r"#\[\s*ignore\s*\]")
NEXT_FN = re.compile(r"\bfn\s+(\w+)")
RAW_STRING = re.compile(r'b?r(#*)"')
ASSERTION = re.compile(
    r"\b(?:\w+_)?assert(?:_\w+)?\s*!"  # assert!, assert_eq!, prop_assert!, ...
    r"|\bassert_\w+\s*\("  # local assertion helpers
    r"|\.\s*(?:expect_err|unwrap_err)\s*\("
    r"|\b(?:panic|unreachable)\s*!"
)
FOCUSED = re.compile(r"\b(?:test|it|describe)\.only\s*\(|\b(?:fdescribe|fit)\s*\(")
BARE_SKIP = re.compile(
    r"^\s*@(?:pytest\.mark\.skip|unittest\.skip)\s*(?:\(\s*\))?\s*$", re.MULTILINE
)
PY_DEF = re.compile(r"\b(?:def|class)\s+(\w+)")

Offence = tuple[str, str, int, str]  # rule, name, line, message


def strip_rust(src: str) -> str:
    """Blank out comments and the contents of string and char literals.

    Length and line breaks are preserved, so offsets in the result are offsets
    in the source.
    """
    out = list(src)
    i, n = 0, len(src)

    def blank(start: int, end: int) -> None:
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        ch = src[i]
        raw = RAW_STRING.match(src, i) if ch in "rb" else None
        if src.startswith("//", i):
            end = src.find("\n", i)
            end = n if end == -1 else end
            blank(i, end)
            i = end
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif ch == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            blank(i + 1, j)
            i = j + 1
        elif raw and not _ident_before(src, i):
            closing = '"' + raw.group(1)
            end = src.find(closing, raw.end())
            end = n if end == -1 else end
            blank(raw.end(), end)
            i = end + len(closing)
        elif ch == "'":
            i = _skip_quote(src, i, blank)
        else:
            i += 1
    return "".join(out)


def _skip_quote(src: str, i: int, blank) -> int:
    """Blank a char literal starting at `i`; leave a lifetime alone."""
    n = len(src)
    if i + 1 < n and src[i + 1] == "\\":
        j = src.find("'", i + 3)
        j = n if j == -1 else j
        blank(i + 1, j)
        return j + 1
    if i + 2 < n and src[i + 2] == "'":
        blank(i + 1, i + 2)
        return i + 3
    return i + 1


def _ident_before(src: str, i: int) -> bool:
    return i > 0 and (src[i - 1].isalnum() or src[i - 1] == "_")


def _matching_brace(text: str, open_at: int) -> int:
    depth = 0
    for k in range(open_at, len(text)):
        if text[k] == "{":
            depth += 1
        elif text[k] == "}":
            depth -= 1
            if depth == 0:
                return k
    return len(text)


def _attr_block_start(text: str, attr_start: int) -> int:
    """Offset of the first line of the attribute block containing `attr_start`."""
    start = text.rfind("\n", 0, attr_start) + 1
    while start > 0:
        prev = text.rfind("\n", 0, start - 1) + 1
        if not text[prev:start].strip().startswith("#["):
            break
        start = prev
    return start


def _documented(src: str, block_start: int) -> bool:
    """True when the line above the attribute block is a comment."""
    above = src[:block_start].rstrip("\n").rsplit("\n", 1)[-1]
    return above.strip().startswith("//")


def _line(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def check_rust(src: str) -> list[Offence]:
    """Return the offences in one Rust source file."""
    text = strip_rust(src)
    found = []
    for match in BARE_IGNORE.finditer(text):
        name = NEXT_FN.search(text, match.end())
        found.append(
            (
                "bare-ignore",
                name.group(1) if name else "?",
                _line(text, match.start()),
                '#[ignore] needs a reason: #[ignore = "why, and which job runs it"]',
            )
        )
    for match in TEST_ATTR.finditer(text):
        block_start = _attr_block_start(text, match.start())
        pos = match.end()
        while more := ATTR.match(text, pos):
            pos = more.end()
        attrs = text[block_start:pos]
        fn = FN.match(text, pos)
        if not fn:
            continue
        name = fn.group(1)
        open_at = text.find("{", fn.end())
        if open_at == -1:
            continue
        body = text[open_at : _matching_brace(text, open_at) + 1]
        if "should_panic" in attrs or ASSERTION.search(body):
            continue
        if name.endswith("_does_not_panic") and _documented(src, block_start):
            continue
        found.append(
            ("no-assert", name, _line(text, match.start()), "test asserts nothing")
        )
    return found


def check_js(src: str) -> list[Offence]:
    """Return focused-test offences in one JavaScript or TypeScript file."""
    return [
        (
            "focused-test",
            m.group(0).rstrip("( \t"),
            _line(src, m.start()),
            "focused test must not be committed",
        )
        for m in FOCUSED.finditer(src)
    ]


def check_python(src: str) -> list[Offence]:
    """Return reasonless skips in one Python test file."""
    found = []
    for m in BARE_SKIP.finditer(src):
        name = PY_DEF.search(src, m.end())
        found.append(
            (
                "bare-skip",
                name.group(1) if name else "?",
                _line(src, m.start()),
                "skip needs a reason",
            )
        )
    return found


def _checker(path: Path):
    if path.suffix == ".rs":
        return check_rust
    if path.suffix in JS_SUFFIXES:
        return check_js
    if path.suffix == ".py" and path.name.startswith(("test_", "test-")):
        return check_python
    return None


def scan(root: Path) -> list[tuple[str, str, int, str]]:
    """Return (key, rule, line, message) for every offence under `root`."""
    offences = []
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root)
        check = _checker(path)
        if check is None or SKIP_DIRS.intersection(rel.parts) or not path.is_file():
            continue
        try:
            src = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for rule, name, line, message in check(src):
            offences.append((f"{rel.as_posix()}::{name}", rule, line, message))
    return offences


def load_allowlist(path: Path) -> set[str]:
    """Read `path::name  # reason` lines; a missing file is an empty list."""
    if not path.exists():
        return set()
    entries = set()
    for raw in path.read_text(encoding="utf-8").splitlines():
        entry = raw.split("#", 1)[0].strip()
        if entry:
            entries.add(entry)
    return entries


def run(root: Path, allowlist: Path, out=sys.stdout) -> int:
    """Print failures and stale allowlist entries; return the exit code."""
    allowed = load_allowlist(allowlist)
    offences = scan(root)
    failures = [o for o in offences if o[0] not in allowed]
    stale = sorted(allowed - {o[0] for o in offences})
    for key, rule, line, message in failures:
        path, name = key.split("::", 1)
        print(f"{path}:{line}: {rule}: {name}: {message}", file=out)
    for entry in stale:
        print(f"note: allowlist entry no longer needed: {entry}", file=out)
    if failures:
        print(
            f"{len(failures)} test hygiene failure(s). See docs/guides/testing.md.",
            file=out,
        )
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--allowlist", type=Path, default=ALLOWLIST)
    parser.add_argument(
        "--list",
        action="store_true",
        help="print every offence key, ignoring the allowlist",
    )
    args = parser.parse_args()
    if args.list:
        for key, rule, _line_no, _message in scan(args.root):
            print(f"{key}  # {rule}")
        return 0
    return run(args.root, args.allowlist)


if __name__ == "__main__":
    sys.exit(main())

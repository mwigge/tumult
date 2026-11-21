#!/usr/bin/env python3
"""Cheap test-hygiene guard. See CONTRIBUTING.md.

Fails on:
  no-assert      a Rust test whose body asserts nothing
  bare-ignore    `#[ignore]` or `#[cfg_attr(.., ignore)]` without a reason
  bare-sleep     `sleep`, `sleep_until` or `park_timeout` in Rust test code
                 without a `// sleep: why` comment of at least three words
  raw-env        `env::set_var` / `env::remove_var` in Rust test code
  focused-test   `test.only` / `it.only` / `describe.only` / `fdescribe` / `fit`
  bare-skip      a Python skip or xfail without a reason
  stale-allow    an allowlist entry that matches nothing
  bad-allow      an allowlist entry that is not `path::name::rule`

Known offenders in crates that are being changed elsewhere are listed in
scripts/test-hygiene-allow.txt, one `path::name::rule  # reason` per line.

This is a text scan, not a parser. Its limits:
  - It cannot judge whether an assertion is meaningful. It only rejects an
    assertion on literals alone, such as `assert!(true)` or `assert_eq!(1, 1)`,
    and does not count `debug_assert!`.
  - A call to `assert_*(..)` counts only when a function of that name is
    defined in the same top-level directory, or in `tumult-test-utils`, and
    itself asserts. Functions are matched by name alone, so two helpers with
    one name are not told apart.
  - Assertions hidden in a helper that is not named `assert_*` are not seen.
  - Test code is a `#[test]`-like function, a `#[cfg(test)] mod` block, a file
    under a `tests` directory or a file named `tests.rs`. Other files pulled
    in with `#[cfg(test)] mod x;` are not checked for sleeps or environment
    changes.
  - Renamed imports are followed for `use` items only. A function taken as
    a value (`let nap = sleep;`, `map(env::remove_var)`) is not seen, and a
    closure that sleeps or changes the environment is reported whether or not
    it is called.
  - A name is resolved per file, not per module: a file that imports
    `std::thread::sleep` treats every bare `sleep(..)` as a thread sleep.
  - JavaScript regular expression literals are not parsed.
"""

from __future__ import annotations

import argparse
import ast
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALLOWLIST = Path(__file__).with_name("test-hygiene-allow.txt")
SKIP_DIRS = {".git", ".claude", "target", "node_modules", "dist", ".quality-venv"}
# The crate of test helpers that every other crate may use.
SHARED_TEST_CRATE = "tumult-test-utils"
JS_SUFFIXES = {".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx"}
RULES = {
    "no-assert",
    "bare-ignore",
    "bare-sleep",
    "raw-env",
    "focused-test",
    "bare-skip",
}
# The one crate that may change the environment directly: it defines EnvGuard.
ENV_OWNER = "tumult-test-utils/"

TEST_ATTR_NAMES = {"test", "rstest", "test_case"}
ATTR_PATH = re.compile(r"\s*((?:\w+\s*::\s*)*\w+)")
FN = re.compile(
    r"(?:pub(?:\s*\([^)]*\))?\s+)?"
    r'(?:(?:const|async|unsafe|extern(?:\s*"[^"]*")?)\s+)*'
    r"fn\s+(\w+)"
)
ANY_FN = re.compile(r"\bfn\s+(\w+)")
MOD = re.compile(r"(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+\w+\s*\{")
CFG_TEST = re.compile(r"\s*cfg\s*\((?:.*\W)?test\b", re.DOTALL)
IGNORE_WORD = re.compile(r"\bignore\b")
IGNORE_REASON = re.compile(r'\s*=\s*"\s*[^\s"]')
RAW_STRING = re.compile(r'b?r(#*)"')
ASSERT_MACRO = re.compile(r"\b((?:\w+_)?assert(?:_\w+)?)\s*!\s*[(\[{]")
# An identifier; the suffix of a literal such as `1u8` does not match.
IDENTIFIER = re.compile(r"(?<![\w.])[A-Za-z_]\w*")
# How many leading arguments a macro compares; the rest is the message.
COMPARED_ARGS = {"assert": 1, "assert_eq": 2, "assert_ne": 2}
# Fails the test by itself: error unwraps, panics, and the asserting methods
# of clap (`debug_assert`), mockito and assert_cmd (`assert`, `assert_async`).
ASSERTION = re.compile(
    r"\.\s*(?:expect_err|unwrap_err|debug_assert|assert|assert_async)\s*\("
    r"|\b(?:panic|unreachable)\s*!"
)
RESULT_ASSERTION = re.compile(r"\b(?:ensure|bail)\s*!|\breturn\s+Err\b")
HELPER_CALL = re.compile(r"\b(assert_\w+)\s*\(")
USE = re.compile(r"\buse\s+([^;]+);")
USE_ITEM = re.compile(r"\b(\w+)(?:\s+as\s+(\w+))?\s*(?=[,}]|$)")
QUALIFIER = re.compile(r"(\w+)\s*::\s*$")
SLEEP_NAMES = {"sleep", "sleep_until", "park_timeout"}
SLEEP_REASON = re.compile(r"//\s*sleep:(.*)")
SLEEP_REASON_WORDS = 3
PAUSED_ATTR = re.compile(r"\bstart_paused\s*=\s*true\b")
PAUSE_CALL = re.compile(r"\btime::pause\s*\(")
ENV_CHANGES = {"set_var", "remove_var"}
FOCUSED = re.compile(
    r"\b(?P<only>(?:test|it|describe)\.only)\b\s*(?=[(.`])"
    r"|(?<![\w.$])(?P<jasmine>fdescribe|fit)\s*\(\s*['\"`]"
)
# `describe["only"]`, with the string blanked out by strip_js.
FOCUSED_INDEX = re.compile(r"\b(test|it|describe)\s*\[\s*(['\"`])\s{4}\2\s*\]")
# Index of the positional reason argument; None when it is keyword-only.
PY_SKIP_REASON_AT = {
    "skip": 0,
    "skipIf": 1,
    "skipUnless": 1,
    "skipif": None,
    "xfail": None,
}
PY_SKIP_CALLS = {"pytest.skip": 0, "pytest.xfail": 0, "self.skipTest": 0}

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


def _matching(text: str, open_at: int) -> int:
    """Offset of the bracket closing the one at `open_at`, or the text length."""
    opener = text[open_at]
    closer = {"(": ")", "[": "]", "{": "}"}[opener]
    depth = 0
    for k in range(open_at, len(text)):
        if text[k] == opener:
            depth += 1
        elif text[k] == closer:
            depth -= 1
            if depth == 0:
                return k
    return len(text)


def _line(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def _body_open(text: str, pos: int) -> int | None:
    """Offset of the `{` opening the body of the function whose name ends at
    `pos`, or None for a declaration without a body."""
    depth = 0
    for k in range(pos, len(text)):
        ch = text[k]
        if ch in "([":
            depth += 1
        elif ch in ")]":
            depth -= 1
        elif depth == 0 and ch == "{":
            return k
        elif depth == 0 and ch == ";":
            return None
    return None


class _Fn:
    """One function with a body: its name, signature and body span."""

    def __init__(self, text: str, match: re.Match, open_at: int):
        self.name = match.group(1)
        self.name_at = match.start(1)
        self.open_at = open_at
        self.close_at = _matching(text, open_at)
        self.signature = text[match.end(1) : open_at]
        self.body = text[open_at : self.close_at + 1]

    def returns_result(self) -> bool:
        return "->" in self.signature and "Result" in self.signature

    def contains(self, offset: int) -> bool:
        return self.open_at < offset <= self.close_at


def _functions(text: str) -> list[_Fn]:
    found = []
    for match in ANY_FN.finditer(text):
        open_at = _body_open(text, match.end())
        if open_at is not None:
            found.append(_Fn(text, match, open_at))
    return found


def _arguments(args: str) -> list[str]:
    """Split macro arguments at the commas outside any bracket."""
    parts, depth, start = [], 0, 0
    for k, ch in enumerate(args):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append(args[start:k])
            start = k + 1
    parts.append(args[start:])
    return parts


def _names_a_value(expression: str) -> bool:
    """True when `expression` refers to anything but literals."""
    names = IDENTIFIER.findall(expression)
    return any(name not in ("true", "false") for name in names)


def _real_assert_macro(body: str) -> bool:
    """True when `body` has an assert macro that looks at a value.

    `assert!(true)`, `assert!(1 == 1)` and `assert_eq!("a", "a")` do not, and
    `debug_assert!` is compiled out of a release test run.
    """
    for match in ASSERT_MACRO.finditer(body):
        name = match.group(1)
        if name.startswith("debug_assert"):
            continue
        open_at = match.end() - 1
        args = _arguments(body[open_at + 1 : _matching(body, open_at)])
        compared = args[: COMPARED_ARGS.get(name, len(args))]
        if any(_names_a_value(arg) for arg in compared):
            return True
    return False


def _asserts(fn: _Fn, helpers: frozenset[str] | set[str]) -> bool:
    if _real_assert_macro(fn.body) or ASSERTION.search(fn.body):
        return True
    if fn.returns_result() and RESULT_ASSERTION.search(fn.body):
        return True
    return any(call.group(1) in helpers for call in HELPER_CALL.finditer(fn.body))


def _asserting_helpers(
    texts: list[str], shared: frozenset[str] = frozenset()
) -> frozenset[str]:
    candidates = [
        fn for text in texts for fn in _functions(text) if fn.name.startswith("assert_")
    ]
    helpers: set[str] = set(shared)
    grew = True
    while grew:
        grew = False
        for fn in candidates:
            if fn.name not in helpers and _asserts(fn, helpers):
                helpers.add(fn.name)
                grew = True
    return frozenset(helpers)


def asserting_helpers(sources: list[str]) -> frozenset[str]:
    """Names of the `assert_*` functions in `sources` that assert something."""
    return _asserting_helpers([strip_rust(src) for src in sources])


class _Block:
    """A run of attributes; the item they belong to starts at `end`."""

    def __init__(self, text: str, start: int):
        self.text = text
        self.start = start
        self.attrs: list[tuple[str, int, int]] = []  # last path segment, content span
        pos = start
        while text.startswith("#[", pos):
            close = _matching(text, pos + 1)
            path = ATTR_PATH.match(text, pos + 2, close)
            if path:
                name = path.group(1).split("::")[-1].strip()
                self.attrs.append((name, pos + 2, close))
            pos = close + 1
            while pos < len(text) and text[pos].isspace():
                pos += 1
        self.end = pos

    def has(self, name: str) -> bool:
        return any(found == name for found, _start, _end in self.attrs)

    def content(self, name: str) -> str:
        """The text of every attribute called `name`."""
        return " ".join(
            self.text[start:end] for found, start, end in self.attrs if found == name
        )

    def first_test_attr(self) -> int | None:
        for name, start, _end in self.attrs:
            if name in TEST_ATTR_NAMES:
                return start
        return None

    def is_paused(self) -> bool:
        return any(
            PAUSED_ATTR.search(self.text, start, end)
            for _name, start, end in self.attrs
        )

    def bare_ignores(self, src: str) -> list[int]:
        """Offsets of the `ignore` attributes that carry no reason."""
        found = []
        for name, start, end in self.attrs:
            if name not in ("ignore", "cfg_attr"):
                continue
            for word in IGNORE_WORD.finditer(self.text, start, end):
                if not IGNORE_REASON.match(src, word.end(), end):
                    found.append(start)
        return found


def _blocks(text: str) -> list[_Block]:
    found, pos = [], 0
    while (start := text.find("#[", pos)) != -1:
        block = _Block(text, start)
        found.append(block)
        pos = max(block.end, start + 2)
    return found


def _names_the_panic(src: str, block_start: int) -> bool:
    """True when the comment directly above the attribute block mentions a panic."""
    line_start = src.rfind("\n", 0, block_start) + 1
    lines = src[:line_start].splitlines()
    comment = []
    while lines and lines[-1].strip().startswith("//"):
        comment.append(lines.pop())
    return "panic" in "\n".join(comment).lower()


def _parent_module(tree: str, at: int) -> str:
    """The module that the `use` item at `at` is imported from."""
    before = tree[:at].rstrip()
    if not before.endswith("::"):
        depth = 0
        for k in range(len(before) - 1, -1, -1):
            if before[k] == "}":
                depth += 1
            elif before[k] == "{" and depth == 0:
                before = before[:k]
                break
            elif before[k] == "{":
                depth -= 1
        else:
            return ""
    parent = QUALIFIER.search(before)
    return parent.group(1) if parent else ""


def _imports(text: str) -> list[tuple[str, str, str]]:
    """(parent module, item, local name) for every item a `use` brings in."""
    found = []
    for use in USE.finditer(text):
        tree = use.group(1)
        for item in USE_ITEM.finditer(tree):
            local = item.group(2) or item.group(1)
            found.append((_parent_module(tree, item.start()), item.group(1), local))
    return found


def _calls(names: set[str]) -> re.Pattern:
    """A call of one of `names` that is not a method call."""
    return re.compile(
        r"(?<![\w.])(" + "|".join(sorted(map(re.escape, names))) + r")\s*\("
    )


class _Names:
    """What the sleep and environment functions are called in one file."""

    def __init__(self, text: str):
        imports = _imports(text)

        def locals_of(items: set[str], parents: set[str] | None = None) -> set[str]:
            return {
                local
                for parent, item, local in imports
                if item in items and (parents is None or parent in parents)
            }

        self.thread_modules = {"thread"} | locals_of({"thread"})
        self.thread_sleeps = locals_of(SLEEP_NAMES, self.thread_modules)
        self.sleep = _calls(SLEEP_NAMES | locals_of(SLEEP_NAMES))
        env_modules = {"env"} | locals_of({"env"})
        self.env = re.compile(
            r"\b(?:"
            + "|".join(sorted(map(re.escape, env_modules)))
            + r")\s*::\s*(?:set_var|remove_var)\s*\("
        )
        imported = locals_of(ENV_CHANGES, env_modules)
        self.env_imported = _calls(imported) if imported else None

    def blocks_the_thread(self, text: str, match: re.Match) -> bool:
        """True for a sleep that a paused tokio clock does not skip."""
        if match.group(1) == "park_timeout":
            return True
        qualifier = QUALIFIER.search(text, max(0, match.start() - 80), match.start())
        if qualifier:
            return qualifier.group(1) in self.thread_modules
        return match.group(1) in self.thread_sleeps

    def env_changes(self, text: str) -> list[int]:
        """Offsets of the calls that change the process environment."""
        found = [match.start() for match in self.env.finditer(text)]
        if self.env_imported:
            found += [
                match.start()
                for match in self.env_imported.finditer(text)
                if not _is_definition(text, match.start())
                and not QUALIFIER.search(
                    text, max(0, match.start() - 80), match.start()
                )
            ]
        return sorted(found)


def _is_definition(text: str, at: int) -> bool:
    return text[:at].rstrip().endswith("fn")


def _is_reason(comment_line: str) -> bool:
    """True when the line carries a `// sleep:` marker followed by a reason of
    at least a few words."""
    marker = SLEEP_REASON.search(comment_line)
    if not marker:
        return False
    words = [w for w in marker.group(1).split() if any(c.isalpha() for c in w)]
    return len(words) >= SLEEP_REASON_WORDS


def _sleep_explained(lines: list[str], line: int) -> bool:
    """True when line number `line`, or the comment directly above it, carries
    a `// sleep: why` marker."""
    if _is_reason(lines[line - 1]):
        return True
    above = line - 2
    while above >= 0 and lines[above].strip().startswith("//"):
        if _is_reason(lines[above]):
            return True
        above -= 1
    return False


def _is_test_file(path: str) -> bool:
    parts = path.split("/")
    return "tests" in parts[:-1] or parts[-1] == "tests.rs"


def _in_spans(spans: list[tuple[int, int]], offset: int) -> bool:
    return any(start < offset <= end for start, end in spans)


def _innermost(functions: list[_Fn], offset: int) -> _Fn | None:
    inside = [fn for fn in functions if fn.contains(offset)]
    return max(inside, key=lambda fn: fn.open_at, default=None)


def _check_rust(
    src: str, text: str, path: str, helpers: frozenset[str]
) -> list[Offence]:
    functions = _functions(text)
    by_name_at = {fn.name_at: fn for fn in functions}
    found: list[Offence] = []
    test_spans: list[tuple[int, int]] = []
    paused_spans = [
        (fn.open_at, fn.close_at) for fn in functions if PAUSE_CALL.search(fn.body)
    ]

    for block in _blocks(text):
        fn_match = FN.match(text, block.end)
        fn = by_name_at.get(fn_match.start(1)) if fn_match else None
        for at in block.bare_ignores(src):
            owner = fn_match or ANY_FN.search(text, block.end)
            found.append(
                (
                    "bare-ignore",
                    owner.group(1) if owner else "?",
                    _line(text, at),
                    'ignore needs a reason: #[ignore = "why, and which job runs it"]',
                )
            )
        if CFG_TEST.match(block.content("cfg")):
            mod = MOD.match(text, block.end)
            if mod:
                test_spans.append((mod.end() - 1, _matching(text, mod.end() - 1)))
        test_attr = block.first_test_attr()
        if test_attr is None or fn is None:
            continue
        test_spans.append((fn.open_at, fn.close_at))
        if block.is_paused():
            paused_spans.append((fn.open_at, fn.close_at))
        if block.has("should_panic") or "=>" in block.content("test_case"):
            continue
        if _asserts(fn, helpers):
            continue
        if fn.name.endswith("_does_not_panic") and _names_the_panic(src, block.start):
            continue
        found.append(
            ("no-assert", fn.name, _line(text, test_attr), "test asserts nothing")
        )

    whole_file = _is_test_file(path)
    lines = src.splitlines()
    seen: set[tuple[str, int]] = set()

    def report(rule: str, offset: int, message: str) -> None:
        if not (whole_file or _in_spans(test_spans, offset)):
            return
        fn = _innermost(functions, offset)
        key = (rule, fn.open_at if fn else -1)
        if key not in seen:
            seen.add(key)
            found.append((rule, fn.name if fn else "?", _line(text, offset), message))

    names = _Names(text)
    for match in names.sleep.finditer(text):
        at = match.start()
        if _is_definition(text, at) or _sleep_explained(lines, _line(text, at)):
            continue
        if _in_spans(paused_spans, at) and not names.blocks_the_thread(text, match):
            continue
        report(
            "bare-sleep",
            at,
            "wait on the event or a paused clock, or explain with `// sleep: why`",
        )
    if not path.startswith(ENV_OWNER):
        for at in names.env_changes(text):
            report("raw-env", at, "use tumult_test_utils::EnvGuard")
    return sorted(found, key=lambda offence: (offence[2], offence[0]))


def check_rust(
    src: str, path: str = "", helpers: frozenset[str] | None = None
) -> list[Offence]:
    """Return the offences in one Rust source file.

    `path` is the file's path relative to the repository root; it decides
    whether the whole file is test code. `helpers` are the asserting
    `assert_*` functions of the crate; by default those defined in `src`.
    """
    text = strip_rust(src)
    if helpers is None:
        helpers = _asserting_helpers([text])
    return _check_rust(src, text, path, helpers)


def strip_js(src: str) -> str:
    """Blank out comments and the contents of string and template literals."""
    out = list(src)
    i, n = 0, len(src)

    def blank(start: int, end: int) -> None:
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        ch = src[i]
        if src.startswith("//", i):
            end = src.find("\n", i)
            end = n if end == -1 else end
            blank(i, end)
            i = end
        elif src.startswith("/*", i):
            end = src.find("*/", i + 2)
            end = n if end == -1 else end + 2
            blank(i, end)
            i = end
        elif ch in "'\"`":
            j = i + 1
            while j < n and src[j] != ch and (ch == "`" or src[j] != "\n"):
                j += 2 if src[j] == "\\" else 1
            blank(i + 1, j)
            i = j + 1
        else:
            i += 1
    return "".join(out)


def check_js(src: str) -> list[Offence]:
    """Return focused-test offences in one JavaScript or TypeScript file."""
    text = strip_js(src)
    focused = [
        (m.start(), m.group("only") or m.group("jasmine"))
        for m in FOCUSED.finditer(text)
    ]
    focused += [
        (m.start(), f"{m.group(1)}.only")
        for m in FOCUSED_INDEX.finditer(text)
        if src[m.end(2) : m.end(2) + 4] == "only"
    ]
    return [
        ("focused-test", name, _line(text, at), "focused test must not be committed")
        for at, name in sorted(focused)
    ]


def _has_reason(call: ast.Call | None, position: int | None) -> bool:
    """True when the skip carries a reason that is not an empty string."""
    if call is None:
        return False
    reason = next(
        (kw.value for kw in call.keywords if kw.arg in ("reason", "msg")), None
    )
    if reason is None and position is not None and len(call.args) > position:
        reason = call.args[position]
    if reason is None:
        return False
    return not (isinstance(reason, ast.Constant) and not str(reason.value).strip())


def _bare_marks(node: ast.expr) -> list[str]:
    """The skips and expected failures in `node` that carry no reason.

    `node` is a decorator, a mark, or a list or tuple of marks.
    """
    if isinstance(node, (ast.List, ast.Tuple)):
        return [kind for element in node.elts for kind in _bare_marks(element)]
    call = node if isinstance(node, ast.Call) else None
    kind = ast.unparse(call.func if call else node).rsplit(".", 1)[-1]
    if kind not in PY_SKIP_REASON_AT or _has_reason(call, PY_SKIP_REASON_AT[kind]):
        return []
    return [kind]


class _SkipVisitor(ast.NodeVisitor):
    def __init__(self) -> None:
        self.found: list[Offence] = []
        self.scope = ["?"]

    def _report(self, name: str, line: int, what: str) -> None:
        self.found.append(("bare-skip", name, line, f"{what} needs a reason"))

    def _definition(self, node) -> None:
        self.scope.append(node.name)
        for decorator in node.decorator_list:
            for kind in _bare_marks(decorator):
                self._report(node.name, decorator.lineno, kind)
            self.visit(decorator)
        for child in node.body:
            self.visit(child)
        self.scope.pop()

    def visit_Assign(self, node: ast.Assign) -> None:  # noqa: N802 - ast visitor name
        targets = [t.id for t in node.targets if isinstance(t, ast.Name)]
        if "pytestmark" in targets:
            owner = self.scope[-1] if len(self.scope) > 1 else "pytestmark"
            for kind in _bare_marks(node.value):
                self._report(owner, node.lineno, kind)
        self.generic_visit(node)

    def visit_FunctionDef(self, node) -> None:  # noqa: N802 - ast visitor name
        self._definition(node)

    def visit_AsyncFunctionDef(self, node) -> None:  # noqa: N802 - ast visitor name
        self._definition(node)

    def visit_ClassDef(self, node) -> None:  # noqa: N802 - ast visitor name
        self._definition(node)

    def visit_Call(self, node: ast.Call) -> None:  # noqa: N802 - ast visitor name
        name = ast.unparse(node.func)
        if name in PY_SKIP_CALLS and not _has_reason(node, PY_SKIP_CALLS[name]):
            self._report(self.scope[-1], node.lineno, name)
        if name in ("pytest.param", "param"):
            marks = [kw.value for kw in node.keywords if kw.arg == "marks"]
            for kind in (kind for value in marks for kind in _bare_marks(value)):
                self._report(self.scope[-1], node.lineno, kind)
        self.generic_visit(node)


def check_python(src: str) -> list[Offence]:
    """Return reasonless skips and expected failures in one Python test file.

    A file that does not parse reports nothing: it fails its own test run.
    """
    try:
        tree = ast.parse(src)
    except SyntaxError:
        return []
    visitor = _SkipVisitor()
    visitor.visit(tree)
    return sorted(visitor.found, key=lambda offence: offence[2])


def _is_python_test(path: Path) -> bool:
    return path.suffix == ".py" and (
        path.name.startswith(("test_", "test-")) or path.stem.endswith("_test")
    )


def _read(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return None


def scan(root: Path) -> list[tuple[str, str, int, str]]:
    """Return (key, rule, line, message) for every offence under `root`.

    The key is `path::name::rule`, the form used in the allowlist.
    """
    rust: dict[str, list[tuple[str, str, str]]] = {}  # crate: (path, source, stripped)
    offences = []

    def add(rel: str, found: list[Offence]) -> None:
        for rule, name, line, message in found:
            offences.append((f"{rel}::{name}::{rule}", rule, line, message))

    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root)
        is_js = path.suffix in JS_SUFFIXES
        if not (path.suffix == ".rs" or is_js or _is_python_test(path)):
            continue
        if SKIP_DIRS.intersection(rel.parts) or not path.is_file():
            continue
        src = _read(path)
        if src is None:
            continue
        if path.suffix == ".rs":
            crate = rel.parts[0] if len(rel.parts) > 1 else ""
            rust.setdefault(crate, []).append((rel.as_posix(), src, strip_rust(src)))
        else:
            add(rel.as_posix(), check_js(src) if is_js else check_python(src))
    shared = _asserting_helpers(
        [text for _rel, _src, text in rust.get(SHARED_TEST_CRATE, [])]
    )
    for files in rust.values():
        helpers = _asserting_helpers([text for _rel, _src, text in files], shared)
        for rel, src, text in files:
            add(rel, _check_rust(src, text, rel, helpers))
    return sorted(offences, key=lambda offence: (offence[0].split("::")[0], offence[2]))


def load_allowlist(path: Path) -> list[tuple[int, str]]:
    """Read (line number, entry) pairs; a missing file is an empty list."""
    if not path.exists():
        return []
    entries = []
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        entry = raw.split("#", 1)[0].strip()
        if entry:
            entries.append((number, entry))
    return entries


def _well_formed(entry: str) -> bool:
    parts = entry.split("::")
    return len(parts) == 3 and all(parts) and parts[2] in RULES


def run(root: Path, allowlist: Path, out=sys.stdout) -> int:
    """Print the offences that are not allowlisted and the allowlist entries
    that are stale or malformed; return the exit code."""
    entries = load_allowlist(allowlist)
    allowed = {entry for _number, entry in entries}
    offences = scan(root)
    present = {key for key, _rule, _line_no, _message in offences}
    failures = 0
    for key, rule, line, message in offences:
        if key in allowed:
            continue
        path, name, _rule = key.split("::")
        print(f"{path}:{line}: {rule}: {name}: {message}", file=out)
        failures += 1
    for number, entry in entries:
        if not _well_formed(entry):
            problem = "bad-allow", "expected path::name::rule with a known rule"
        elif entry not in present:
            problem = "stale-allow", "matches nothing, delete the line"
        else:
            continue
        print(
            f"{allowlist.name}:{number}: {problem[0]}: {entry}: {problem[1]}", file=out
        )
        failures += 1
    if failures:
        print(
            f"{failures} test hygiene failure(s). See CONTRIBUTING.md.",
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
        for key in dict.fromkeys(key for key, *_rest in scan(args.root)):
            print(key)
        return 0
    return run(args.root, args.allowlist)


if __name__ == "__main__":
    sys.exit(main())

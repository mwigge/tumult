#!/usr/bin/env python3
"""Guard for structured logging: every tracing event has a constant message.

A log message is a constant; everything that varies is a field. This script
reads the Rust sources of the workspace and fails when a `tracing` event
macro (`trace!`, `debug!`, `info!`, `warn!`, `error!`, `event!`, written
with any of the three delimiters: `info!(..)`, `info! {..}`, `info![..]`):

* interpolated-message  builds its message with format arguments or an
                        inline `{capture}`, or passes a computed `message`
                        field (`message = %format!(..)`);
* missing-event-name    has no stable `name: "component.thing.happened"`;
* bad-event-name        has a name that is not lowercase dotted;
* legacy-field          uses a field name the vocabulary replaced
                        (`error` -> `error.message`, `run_id` ->
                        `resilience.run.id`, ...).

Test code is not checked: `tests/` and `benches/` directories, `tests.rs`
files, and items behind `#[cfg(test)]`.

Crates listed in DEFERRED are not converted yet and are skipped entirely.
Every run prints that list with its size, so the exclusion stays visible.
See docs/guides/logging.md for the vocabulary.

usage: check-log-messages.py [--root DIR] [--self-test] [FILE ...]
"""

import argparse
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path

# TODO(structured-logging-phase-2): convert these crates and delete their
# entries. Each value names the follow-up that removes the exclusion.
DEFERRED = {
    "tumult-api": "structured-logging phase 2: API request spans and events",
    "tumult-compliance": "structured-logging phase 2: compliance renderer events",
    "tumult-core": "structured-logging phase 2: runner events",
    "tumult-graph": "structured-logging phase 2: graph store events",
    "tumult-ingest": "structured-logging phase 2: ingest and run-queue events",
    "tumult-lake": "structured-logging phase 2: lake store events",
    "demo": "structured-logging phase 2: demo services (Rust and Python)",
    "forecastd": "structured-logging phase 2: forecast sidecar",
    "web": "not Rust; browser logging is tracked with the demo follow-up",
}

# Directories that never hold checked sources.
SKIPPED_DIRS = {"target", "node_modules", ".git", ".claude", ".quality-venv"}

EVENT_MACRO = re.compile(
    r"(?<![A-Za-z0-9_])(?:tracing\s*::\s*)?(trace|debug|info|warn|error|event)\s*!\s*[(\[{]"
)
EVENT_NAME = re.compile(r"[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+")
STRING_ARGUMENT = re.compile(r'b?r?#*"[S\n]*"#*')
PREFIX_ARGUMENT = re.compile(r"(name|target|parent)\s*:(?!:)")
CFG_TEST = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")

# Field names the vocabulary replaced, with what to write instead.
LEGACY_FIELDS = {
    "e": "error.message",
    "err": "error.message",
    "error": "error.message",
    "panic": "exception.message",
    "run": "resilience.run.id",
    "run_id": "resilience.run.id",
    "experiment_id": "resilience.experiment.id",
    "experiment_title": "resilience.experiment.name",
    "user": "user.name",
    "username": "user.name",
    "actor": "user.name",
    "client": "client.address",
    "addr": "server.address and server.port",
    "listen_addr": "server.address and server.port",
    "endpoint": "url.full (sanitised) or server.address",
    "url": "url.full (sanitised)",
    "path": "file.path or url.path",
    "file": "file.path",
    "dir": "file.path",
    "db": "file.path",
    "cert": "file.path",
    "script": "file.path",
    "key": "config.key",
    "env_key": "config.key",
    "value": "config.value",
    "pair": "config.key (never the value of a secret)",
    "tool": "gen_ai.tool.name",
    "service": "the service.name resource attribute (not a per-event field)",
    "status": "resilience.outcome.status or http.response.status_code",
    "method": "http.request.method",
}


@dataclass(frozen=True)
class Finding:
    """One rule violation at a source line."""

    path: Path
    line: int
    rule: str
    detail: str

    def render(self):
        return f"{self.path.as_posix()}:{self.line}: {self.rule}: {self.detail}"


@dataclass(frozen=True)
class Argument:
    """A top-level macro argument: masked text, original text."""

    masked: str
    text: str


def _skip_string(source, start):
    """Index just past the string literal whose opening quote is at `start`."""
    index = start + 1
    while index < len(source):
        if source[index] == "\\":
            index += 2
            continue
        if source[index] == '"':
            return index + 1
        index += 1
    return len(source)


def _raw_string_end(source, start):
    """End of a raw string starting at `start` (`r`, `br`), or None."""
    index = start + (2 if source.startswith("br", start) else 1)
    hashes = 0
    while index < len(source) and source[index] == "#":
        hashes += 1
        index += 1
    if index >= len(source) or source[index] != '"':
        return None
    close = source.find('"' + "#" * hashes, index + 1)
    return len(source) if close < 0 else close + 1 + hashes


def _block_comment_end(source, start):
    depth, index = 0, start
    while index < len(source):
        if source.startswith("/*", index):
            depth += 1
            index += 2
        elif source.startswith("*/", index):
            depth -= 1
            index += 2
            if depth == 0:
                return index
        else:
            index += 1
    return len(source)


def _char_literal_end(source, start):
    """End of a char literal at `start`, or None when it is a lifetime."""
    if source.startswith("\\", start + 1):
        close = source.find("'", start + 3)
        return None if close < 0 else close + 1
    if source.startswith("'", start + 2):
        return start + 3
    return None


def _blank(text, keep=""):
    """`text` with everything but newlines (and `keep` chars) blanked."""
    return "".join(c if c == "\n" or c in keep else " " for c in text)


def _mask_string(text):
    """A string literal with its content replaced by `S`, delimiters kept."""
    opening = text.index('"') + 1
    closing = text.rindex('"') if text.rindex('"') >= opening else len(text)
    body = "".join("\n" if c == "\n" else "S" for c in text[opening:closing])
    return text[:opening] + body + text[closing:]


def mask_source(source):
    """`source` with comments blanked and string/char contents replaced.

    The result has the same length and line breaks, so offsets and line
    numbers carry over, but brackets, commas and macro names can only come
    from real code.
    """
    out, index = [], 0
    while index < len(source):
        char = source[index]
        end = None
        if source.startswith("//", index):
            end = source.find("\n", index)
            end = len(source) if end < 0 else end
            out.append(_blank(source[index:end]))
        elif source.startswith("/*", index):
            end = _block_comment_end(source, index)
            out.append(_blank(source[index:end]))
        elif char == '"':
            end = _skip_string(source, index)
            out.append(_mask_string(source[index:end]))
        elif char in "rb" and not _is_identifier_char(source, index - 1):
            end = _prefixed_literal_end(source, index)
            if end is not None:
                out.append(_mask_string(source[index:end]))
        elif char == "'":
            end = _char_literal_end(source, index)
            if end is not None:
                out.append("'" + "C" * (end - index - 2) + "'")
        if end is None:
            out.append(char)
            end = index + 1
        index = end
    return "".join(out)


def _is_identifier_char(source, index):
    return index >= 0 and (source[index].isalnum() or source[index] == "_")


def _prefixed_literal_end(source, start):
    """End of a `b"..."`, `r"..."`, `r#"..."#` or `br"..."` literal, or None."""
    if source.startswith('b"', start):
        return _skip_string(source, start + 1)
    if source.startswith(("r", "br"), start):
        return _raw_string_end(source, start)
    return None


def _matching_close(masked, open_index):
    """Index of the bracket closing the one at `open_index`."""
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = []
    for index in range(open_index, len(masked)):
        char = masked[index]
        if char in pairs:
            stack.append(pairs[char])
        elif stack and char == stack[-1]:
            stack.pop()
            if not stack:
                return index
    return len(masked) - 1


def _split_arguments(masked, text):
    """Split on top-level commas; both strings cover the same span."""
    arguments, depth, start = [], 0, 0
    for index, char in enumerate(masked):
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        elif char == "," and depth == 0:
            arguments.append(Argument(masked[start:index], text[start:index]))
            start = index + 1
    arguments.append(Argument(masked[start:], text[start:]))
    return [a for a in arguments if a.masked.strip()]


def _blank_test_items(masked):
    """Blank every item annotated with `#[cfg(test)]`."""
    for match in reversed(list(CFG_TEST.finditer(masked))):
        brace = masked.find("{", match.end())
        semicolon = masked.find(";", match.end())
        if brace < 0 and semicolon < 0:
            continue
        if brace < 0 or 0 <= semicolon < brace:
            end = semicolon + 1
        else:
            end = _matching_close(masked, brace) + 1
        masked = (
            masked[: match.start()] + _blank(masked[match.start() : end]) + masked[end:]
        )
    return masked


def _literal_content(text):
    """The characters between the quotes of a string literal."""
    stripped = text.strip()
    return stripped[stripped.index('"') + 1 : stripped.rindex('"')]


def _is_interpolated(literal):
    return "{" in literal.replace("{{", "").replace("}}", "")


def _field_arguments(arguments):
    """Field arguments, with `{ a = 1, b }` groups flattened."""
    fields = []
    for argument in arguments:
        stripped = argument.masked.strip()
        if not (stripped.startswith("{") and stripped.endswith("}")):
            fields.append(argument)
            continue
        offset = argument.masked.index("{") + 1
        close = argument.masked.rindex("}")
        fields.extend(
            _split_arguments(argument.masked[offset:close], argument.text[offset:close])
        )
    return fields


def _field_name(argument):
    """The field name of `name = value`, `%name`, `?name` or `name`."""
    masked = argument.masked
    equals = re.search(r"(?<![=!<>])=(?!=)", masked)
    end = equals.start() if equals else len(masked)
    name_masked, name_text = masked[:end].strip(), argument.text[:end].strip()
    if STRING_ARGUMENT.fullmatch(name_masked):
        return _literal_content(name_text)
    return name_text.lstrip("%?").strip()


def _is_computed_message(argument):
    """Whether a field argument is a `message` that is not a string literal.

    `tracing` takes a field called `message` as the event's message, so
    `message = %format!(..)` is an interpolated message by another route.
    """
    if _field_name(argument) != "message":
        return False
    equals = re.search(r"(?<![=!<>])=(?!=)", argument.masked)
    if equals is None:
        return True
    value = argument.masked[equals.end() :].strip()
    return not STRING_ARGUMENT.fullmatch(value)


def _looks_like_level(argument):
    return re.fullmatch(r"(\w+\s*::\s*)*Level\s*::\s*\w+", argument.masked.strip())


@dataclass(frozen=True)
class Event:
    """A parsed event macro call."""

    name: str | None
    message: str | None
    fields: list
    format_arguments: int


def parse_event(masked, text):
    """Parse the argument list of an event macro (without the parentheses)."""
    name, message, rest, extra = None, None, [], 0
    for argument in _split_arguments(masked, text):
        stripped = argument.masked.strip()
        prefix = PREFIX_ARGUMENT.match(stripped)
        if message is not None:
            extra += 1
        elif prefix and prefix.group(1) == "name":
            name = _literal_content(argument.text) if '"' in argument.text else ""
        elif prefix or _looks_like_level(argument):
            continue
        elif STRING_ARGUMENT.fullmatch(stripped):
            message = _literal_content(argument.text)
        else:
            rest.append(argument)
    return Event(name, message, _field_arguments(rest), extra)


def _event_findings(event, strict):
    """`(rule, detail)` pairs for one event."""
    found = []
    if event.format_arguments:
        found.append(
            (
                "interpolated-message",
                "message takes format arguments; use fields instead",
            )
        )
    elif event.message is not None and _is_interpolated(event.message):
        found.append(
            ("interpolated-message", "message captures a variable; use a field instead")
        )
    if any(_is_computed_message(field) for field in event.fields):
        found.append(
            (
                "interpolated-message",
                "`message` field is computed; use a constant message and other fields",
            )
        )
    if not strict:
        return found
    if event.name is None:
        found.append(("missing-event-name", 'add name: "component.thing.happened"'))
    elif not EVENT_NAME.fullmatch(event.name):
        found.append(
            ("bad-event-name", f"{event.name!r} is not lowercase dotted (a.b.c)")
        )
    for field in event.fields:
        name = _field_name(field)
        if name in LEGACY_FIELDS:
            found.append(("legacy-field", f"field `{name}`: use {LEGACY_FIELDS[name]}"))
    return found


def check_source(source, path=Path("<source>"), strict=True):
    """All findings for one Rust source text."""
    masked = _blank_test_items(mask_source(source))
    findings = []
    for match in EVENT_MACRO.finditer(masked):
        open_index = match.end() - 1
        close_index = _matching_close(masked, open_index)
        event = parse_event(
            masked[open_index + 1 : close_index], source[open_index + 1 : close_index]
        )
        line = masked.count("\n", 0, match.start()) + 1
        findings.extend(
            Finding(path, line, rule, detail)
            for rule, detail in _event_findings(event, strict)
        )
    return findings


def is_test_path(path):
    """Whether `path` holds test or bench code only."""
    parts = path.parts
    return "tests" in parts[:-1] or "benches" in parts[:-1] or path.name == "tests.rs"


def rust_sources(root, deferred):
    """Checked Rust files under `root`, relative to it, sorted."""
    sources = []
    for directory, subdirectories, files in os.walk(root):
        relative = Path(directory).relative_to(root)
        at_root = relative == Path(".")
        subdirectories[:] = [
            name
            for name in subdirectories
            if name not in SKIPPED_DIRS and not (at_root and name in deferred)
        ]
        sources.extend(relative / name for name in files if name.endswith(".rs"))
    return sorted(path for path in sources if not is_test_path(path))


def check_tree(root, deferred=None):
    """Findings for every checked Rust file under `root`."""
    deferred = DEFERRED if deferred is None else deferred
    findings = []
    for relative in rust_sources(root, deferred):
        source = (root / relative).read_text(encoding="utf-8")
        findings.extend(check_source(source, relative))
    return findings


SELF_TEST_CASES = [
    ('tracing::info!(name: "a.b", { rows = 3 }, "export complete");', []),
    ('tracing::info!(name: "a.b", "done: {result:?}");', ["interpolated-message"]),
    ('tracing::info!(name: "a.b", "done: {:?}", result);', ["interpolated-message"]),
    ('warn!(name: "a.b", "retry {}", attempt);', ["interpolated-message"]),
    ('tracing::info!("started");', ["missing-event-name"]),
    ('tracing::info!(name: "Started", "started");', ["bad-event-name"]),
    ('tracing::warn!(name: "a.b", error = %e, "failed");', ["legacy-field"]),
    ('// tracing::info!("commented {}", x);', []),
    ('let s = "tracing::info!(\\"quoted {}\\", x)";', []),
    ('#[cfg(test)]\nmod tests { fn t() { tracing::info!("{}", 1); } }', []),
    ('tracing::info! { name: "a.b", "done {}", n }', ["interpolated-message"]),
    ('tracing::info![name: "a.b", "done {n}"];', ["interpolated-message"]),
    (
        'tracing::info!(name: "a.b", message = %format!("done {n}"));',
        ["interpolated-message"],
    ),
]


def self_test():
    """Run the built-in cases; returns the failures as text lines."""
    failures = []
    for source, expected in SELF_TEST_CASES:
        actual = [finding.rule for finding in check_source(source)]
        if actual != expected:
            failures.append(f"{source!r}: expected {expected}, got {actual}")
    return failures


def deferred_notice(deferred=None):
    """The line naming what is skipped, printed on every checking run."""
    deferred = DEFERRED if deferred is None else deferred
    return f"deferred, not checked ({len(deferred)}): {', '.join(sorted(deferred))}"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parent.parent
    )
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("files", nargs="*", type=Path)
    options = parser.parse_args(argv)

    if options.self_test:
        failures = self_test()
        for failure in failures:
            print(f"self-test failed: {failure}")
        print(
            f"self-test: {len(SELF_TEST_CASES) - len(failures)}/{len(SELF_TEST_CASES)} passed"
        )
        return 1 if failures else 0

    print(deferred_notice())
    if options.files:
        findings = []
        for path in options.files:
            findings.extend(check_source(path.read_text(encoding="utf-8"), path))
    else:
        findings = check_tree(options.root)
    for finding in findings:
        print(finding.render())
    if findings:
        print(f"{len(findings)} log message problem(s); see docs/guides/logging.md")
        return 1
    print("log messages ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())

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
                        `resilience.run.id`, ...);
* computed-error-message  builds `error.message` with `format!`,
                        `format_args!` or `concat!`: that is a message
                        with interpolation under another name.

Spans are checked too: the span macros (`span!`, `info_span!`, ...) and
`#[instrument(..)]` attributes, also inside `cfg_attr`:

* bad-span-name         has a name that is not lowercase dotted. A span
                        without a literal name is not reported: an
                        `#[instrument]` without `name = ".."` is named
                        after its function;
* legacy-field and computed-error-message, for the fields of the span.

A macro imported under another name (`use tracing::info as i;`, also in a
`use tracing::{..}` group) is checked under that name in the same file.

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

EVENT_MACROS = frozenset({"trace", "debug", "info", "warn", "error", "event"})
SPAN_MACROS = frozenset(
    {"span", "trace_span", "debug_span", "info_span", "warn_span", "error_span"}
)
MACRO_CALL = re.compile(
    r"(?<![A-Za-z0-9_])(?:tracing\s*::\s*)?([A-Za-z_][A-Za-z0-9_]*)\s*!\s*[(\[{]"
)
TRACING_USE = re.compile(r"(?<![A-Za-z0-9_])use\s+(?:::\s*)?tracing\s*::\s*([^;]*);")
ALIASED = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\s+as\s+([A-Za-z_][A-Za-z0-9_]*)")
ATTRIBUTE = re.compile(r"#\s*!?\s*\[")
INSTRUMENT = re.compile(r"(?<![A-Za-z0-9_])instrument\s*\(")
ASSIGNMENT = re.compile(r"(?<![=!<>])=(?!=)")
TEXT_MACRO = re.compile(
    r"(?<![A-Za-z0-9_])(?:format|format_args|concat)\s*!\s*[(\[{]\s*"
)
PLACEHOLDER = re.compile(r"\{[^{}]*\}")
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
    equals = ASSIGNMENT.search(masked)
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
    value = _field_value(argument)
    return value is None or not STRING_ARGUMENT.fullmatch(value)


def _field_value(argument):
    """The masked value of `name = value`, or None for a shorthand field."""
    equals = ASSIGNMENT.search(argument.masked)
    return None if equals is None else argument.masked[equals.end() :].strip()


def _is_computed_error_message(argument):
    """Whether a field argument is an `error.message` put together from text.

    `error.message = %format!("{name} failed: {e}")` is an interpolated
    message that moved into a field: the constant part belongs in the
    message, the name in a field of its own, and the error is passed as is.
    """
    if _field_name(argument) != "error.message":
        return False
    macro = TEXT_MACRO.search(argument.masked)
    return macro is not None and not _renders_one_value(argument, macro)


def _renders_one_value(argument, macro):
    """Whether a formatting macro call is one placeholder and nothing else.

    `format!("{e:#}")` is the error in another rendering, not a sentence.
    """
    if "concat" in macro.group(0):
        return False
    template = STRING_ARGUMENT.match(argument.masked, macro.end())
    if template is None:
        return False
    literal = _literal_content(argument.text[template.start() : template.end()])
    return len(PLACEHOLDER.findall(literal)) == 1 and not PLACEHOLDER.sub("", literal)


def _field_findings(fields):
    """`(rule, detail)` pairs for the fields of an event or a span."""
    found = []
    for field in fields:
        name = _field_name(field)
        if name in LEGACY_FIELDS:
            found.append(("legacy-field", f"field `{name}`: use {LEGACY_FIELDS[name]}"))
        if _is_computed_error_message(field):
            found.append(
                (
                    "computed-error-message",
                    "`error.message` is formatted text; pass the error itself"
                    " and put what varies in fields of its own",
                )
            )
    return found


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
    return found + _field_findings(event.fields)


@dataclass(frozen=True)
class Span:
    """A parsed span macro call or `#[instrument(..)]` attribute."""

    name: str | None
    fields: list


def parse_span(masked, text):
    """Parse the argument list of a span macro (without the parentheses)."""
    name, rest = None, []
    for argument in _split_arguments(masked, text):
        stripped = argument.masked.strip()
        if PREFIX_ARGUMENT.match(stripped) or _looks_like_level(argument):
            continue
        if name is None and not rest and STRING_ARGUMENT.fullmatch(stripped):
            name = _literal_content(argument.text)
            continue
        rest.append(argument)
    return Span(name, _field_arguments(rest))


def parse_instrument(masked, text):
    """Parse the argument list of `#[instrument(..)]`."""
    name, fields = None, []
    for argument in _split_arguments(masked, text):
        stripped = argument.masked.strip()
        value = _field_value(argument)
        if re.match(r"name\s*=", stripped) and STRING_ARGUMENT.fullmatch(value or ""):
            name = _literal_content(argument.text)
        elif re.match(r"fields\s*\(", stripped):
            start = argument.masked.index("(") + 1
            end = argument.masked.rindex(")")
            fields.extend(
                _split_arguments(argument.masked[start:end], argument.text[start:end])
            )
    return Span(name, fields)


def _span_findings(span):
    """`(rule, detail)` pairs for one span."""
    found = []
    if span.name is not None and not EVENT_NAME.fullmatch(span.name):
        found.append(
            ("bad-span-name", f"{span.name!r} is not lowercase dotted (a.b.c)")
        )
    return found + _field_findings(span.fields)


def _use_items(body):
    """The items of the path after `use tracing::`, one per imported name."""
    body = body.strip()
    if not (body.startswith("{") and body.endswith("}")):
        return [body]
    return [item.masked.strip() for item in _split_arguments(body[1:-1], body[1:-1])]


def macro_names(masked):
    """Every name a tracing macro goes by in this source, to its own name.

    `use tracing::info as i;` makes `i!` an `info!`. Only macros imported
    straight from `tracing` count: `tracing::field::debug as d` is a
    function, and `std::format as info2` is not ours.
    """
    macros = EVENT_MACROS | SPAN_MACROS
    names = {name: name for name in macros}
    for use in TRACING_USE.finditer(masked):
        for item in _use_items(use.group(1)):
            aliased = ALIASED.fullmatch(item)
            if aliased and aliased.group(1) in macros:
                names[aliased.group(2)] = aliased.group(1)
    return names


def _macro_findings(masked, source, strict):
    """`(offset, (rule, detail))` for every event and span macro call."""
    names, located = macro_names(masked), []
    for match in MACRO_CALL.finditer(masked):
        macro = names.get(match.group(1))
        if macro is None:
            continue
        open_index = match.end() - 1
        close_index = _matching_close(masked, open_index)
        arguments = (
            masked[open_index + 1 : close_index],
            source[open_index + 1 : close_index],
        )
        if macro in EVENT_MACROS:
            found = _event_findings(parse_event(*arguments), strict)
        else:
            found = _span_findings(parse_span(*arguments)) if strict else []
        located.extend((match.start(), finding) for finding in found)
    return located


def _instrument_findings(masked, source):
    """`(offset, (rule, detail))` for every `instrument(..)` in an attribute."""
    located = []
    for attribute in ATTRIBUTE.finditer(masked):
        attribute_end = _matching_close(masked, attribute.end() - 1)
        for call in INSTRUMENT.finditer(masked, attribute.end(), attribute_end):
            close_index = _matching_close(masked, call.end() - 1)
            span = parse_instrument(
                masked[call.end() : close_index], source[call.end() : close_index]
            )
            located.extend(
                (attribute.start(), finding) for finding in _span_findings(span)
            )
    return located


def check_source(source, path=Path("<source>"), strict=True):
    """All findings for one Rust source text, in source order."""
    masked = _blank_test_items(mask_source(source))
    located = _macro_findings(masked, source, strict)
    if strict:
        located.extend(_instrument_findings(masked, source))
    located.sort(key=lambda item: item[0])
    return [
        Finding(path, masked.count("\n", 0, offset) + 1, rule, detail)
        for offset, (rule, detail) in located
    ]


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
    ('use tracing::info as i;\nfn f() { i!("started"); }', ["missing-event-name"]),
    ('use tracing::{warn as w};\nfn f() { w!(name: "a.b", "x {}", n); }', ["interpolated-message"]),
    ('let s = tracing::info_span!("db query");', ["bad-span-name"]),
    ('let s = tracing::info_span!("db.query", run_id = %id);', ["legacy-field"]),
    ('#[tracing::instrument(name = "Query", skip(self))]\nfn f() {}', ["bad-span-name"]),
    ('#[tracing::instrument(skip(self))]\nfn f() {}', []),
    (
        'tracing::warn!(name: "a.b", { error.message = %format!("{n}: {e}") }, "failed");',
        ["computed-error-message"],
    ),
    ('tracing::warn!(name: "a.b", { error.message = %err(&e) }, "failed");', []),
    ('tracing::warn!(name: "a.b", error.message = %format!("{e:#}"), "failed");', []),
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

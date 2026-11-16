"""Structured logs: one JSON object per line on stderr.

Every line has the same six keys: ``timestamp`` (RFC 3339, UTC), ``level``,
``logger``, ``event`` (a stable dotted name), ``message`` (a constant
sentence) and ``fields`` (everything that varies). Values are never
interpolated into the message, so a string a client or the model wrote can
only ever be a field: it is cut to ``MAX_STRING`` characters and the JSON
encoder escapes its control characters, which keeps it on its line.

``event`` is how this package logs. Records from other libraries (uvicorn,
the ``warnings`` module) pass through the same formatter: their message
template stays the message and their arguments become ``fields.args``.

``FORECASTD_LOG_FORMAT=text`` swaps the JSON object for one readable line
with the same parts, for a person at a terminal.
"""

from __future__ import annotations

import json
import logging
import math
import os
import re
import sys
import threading
import traceback
from collections.abc import Mapping
from datetime import datetime, timezone
from types import TracebackType
from typing import Any

FORMATS = ("json", "text")
LEVELS = ("DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL")
# The longest string a log line carries, and the longest stack trace.
MAX_STRING = 1024
MAX_STACK = 8192
TRUNCATED = "...[truncated]"
# Entries kept of a list or a mapping, the longest mapping key, and how
# deep a nested value is followed before it is logged as its ``repr``.
MAX_ITEMS = 32
MAX_KEY = 128
MAX_DEPTH = 4
# The largest integers every JSON reader agrees on.
_INT_RANGE = range(-(2**63), 2**64)
UVICORN_LOGGERS = ("uvicorn", "uvicorn.error", "uvicorn.access")
# What a record that did not come through ``event`` is called.
PLAIN_EVENT = "log"
# The line written in place of a record that could not be formatted.
UNFORMATTABLE = "log.unformattable"

ExcInfo = tuple[type[BaseException], BaseException, TracebackType | None]
REDACTED = "[redacted]"
# A URL is whatever follows ``://`` up to the next blank, under any scheme
# and in any case. Each pattern is one character class repeated, and the
# rest is ``str`` methods, so the text is read a fixed number of times
# however it is built.
_SCHEME_END = "://"
_BLANKS = re.compile(r"(\s+)")
_PATH_PARAMETER = re.compile(r";[^/;]*")


def _authority(part: str, cut: bool) -> str:
    """What follows one ``://``, up to the query: host and path, without
    the ``user:password@`` before the host and without ``;name=value``
    path parameters. ``cut`` says the text was cut off right here: with no
    ``/`` yet, what looks like a host may be the first half of a password."""
    host, slash, path = part.partition("/")
    _, at, name = host.rpartition("@")
    if at:
        host = f"{REDACTED}@{name}"
    elif cut and not slash:
        return REDACTED
    return _PATH_PARAMETER.sub(f";{REDACTED}", host + slash + path)


def _word(word: str, cut: bool) -> str:
    """One run of text without blanks, with the URL in it redacted."""
    head, scheme_end, rest = word.partition(_SCHEME_END)
    if not scheme_end:
        return word
    tail = ""
    marks = [at for at in (rest.find("?"), rest.find("#")) if at != -1]
    if marks:
        # A query or a fragment runs to the end of the word: a bracket or
        # a quote may be part of it, and so may a second URL.
        first = min(marks)
        tail = rest[first] + REDACTED
        rest = rest[:first]
        cut = False
    # A path may hold a URL of its own (a proxy, a redirect).
    parts = rest.split(_SCHEME_END)
    last = len(parts) - 1
    kept = [_authority(part, cut and at == last) for at, part in enumerate(parts)]
    return head + _SCHEME_END + _SCHEME_END.join(kept) + tail


def redact_urls(text: str, cut: bool = False) -> str:
    """``text`` with every URL in it reduced to scheme, host and path: the
    credentials, the path parameters, the query and the fragment are
    replaced. An error from a download quotes the URL it failed on.
    ``cut`` says ``text`` is the start of something longer."""
    if _SCHEME_END not in text:
        return text
    words = _BLANKS.split(text)
    last = len(words) - 1
    # Even entries are words, odd ones the blanks between them.
    return "".join(
        _word(word, cut and at == last) if at % 2 == 0 else word for at, word in enumerate(words)
    )


def clip(value: str, limit: int = MAX_STRING) -> str:
    """``value``, cut to ``limit`` characters and marked when it was cut."""
    if len(value) <= limit:
        return value
    return value[:limit] + TRUNCATED


def redacted(value: str, limit: int = MAX_STRING) -> str:
    """``value`` cut to ``limit`` characters, then without what its URLs
    could give away, and marked when it was cut. The cut comes first, so
    the work is bounded by the limit and not by what an error chose to
    say; redacting can lengthen the text, so it is cut once more."""
    cut = len(value) > limit
    out = redact_urls(value[:limit], cut)
    if len(out) > limit:
        cut = True
    return out[:limit] + TRUNCATED if cut else out


def _text(value: object) -> str:
    """``str(value)``, or what went wrong when its ``__str__`` raises."""
    try:
        return str(value)
    except Exception as error:  # noqa: BLE001 - a broken __str__ must not lose the line
        return f"<{type(value).__name__}: str failed with {type(error).__name__}>"


def _described(value: object) -> str:
    """A bounded description of a value JSON has no type for."""
    try:
        return clip(repr(value))
    except Exception as error:  # noqa: BLE001 - a broken __repr__ must not lose the line
        return f"<{type(value).__name__}: repr failed with {type(error).__name__}>"


def _scalar(value: Any) -> Any:
    if value is None or isinstance(value, bool):
        return value
    if isinstance(value, int):
        return value if value in _INT_RANGE else "<integer out of range>"
    if isinstance(value, float):
        return value if math.isfinite(value) else str(value)
    if isinstance(value, str):
        return clip(value)
    if isinstance(value, BaseException):
        return clip(_text(value))
    return _described(value)


def bounded(value: Any, depth: int = 0) -> Any:
    """``value`` as something JSON can hold and a log line can afford:
    strings cut, collections cut, everything else described."""
    if depth < MAX_DEPTH and isinstance(value, Mapping):
        items = list(value.items())[:MAX_ITEMS]
        return {clip(_text(key), MAX_KEY): bounded(item, depth + 1) for key, item in items}
    if depth < MAX_DEPTH and isinstance(value, (list, tuple)):
        return [bounded(item, depth + 1) for item in value[:MAX_ITEMS]]
    return _scalar(value)


def error_fields(error: BaseException, stack: str | None = None) -> dict[str, Any]:
    """The fields that describe an exception, named as the daemon names
    them (``error.type``, ``error.message``). URLs in them keep their
    host and path and lose what could be a secret."""
    fields: dict[str, Any] = {
        "error.type": type(error).__name__,
        "error.message": redacted(_text(error)),
    }
    if stack is not None:
        fields["error.stack"] = redacted(stack, MAX_STACK)
    return fields


def event(
    logger: logging.Logger,
    level: int,
    name: str,
    message: str,
    /,
    *,
    exc_info: bool | ExcInfo = False,
    **fields: Any,
) -> None:
    """Log one event: a stable ``name``, a constant ``message`` and the
    values as ``fields``. ``exc_info`` adds the exception being handled."""
    logger.log(level, message, exc_info=exc_info, extra={"event": name, "fields": fields})


def _timestamp(created: float) -> str:
    moment = datetime.fromtimestamp(created, timezone.utc)
    return moment.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def _record_fields(record: logging.LogRecord) -> dict[str, Any]:
    fields = dict(getattr(record, "fields", None) or {})
    if record.args:
        args = record.args
        fields["args"] = args if isinstance(args, Mapping) else list(args)
    error = record.exc_info[1] if record.exc_info else None
    if error is not None:
        stack = "".join(traceback.format_exception(*record.exc_info))
        fields.update(error_fields(error, stack))
    return bounded(fields)


def entry(record: logging.LogRecord) -> dict[str, Any]:
    """The six parts of a line. The message is the record's template as it
    was written, never the template filled in."""
    return {
        "timestamp": _timestamp(record.created),
        "level": record.levelname,
        "logger": clip(record.name, MAX_KEY),
        "event": clip(_text(getattr(record, "event", PLAIN_EVENT)), MAX_KEY),
        "message": clip(_text(record.msg)),
        "fields": _record_fields(record),
    }


def fallback(record: logging.LogRecord, error: Exception) -> dict[str, Any]:
    """The line for a record that could not be formatted: when, how
    severe, which event and what went wrong, and none of its values."""
    return {
        "timestamp": _timestamp(record.created),
        "level": record.levelname,
        "logger": clip(record.name, MAX_KEY),
        "event": UNFORMATTABLE,
        "message": "a log record could not be formatted",
        "fields": {
            "event": clip(_text(getattr(record, "event", PLAIN_EVENT)), MAX_KEY),
            "error.type": type(error).__name__,
        },
    }


class _Formatter(logging.Formatter):
    """Formats the six parts, and the fallback line when they cannot be
    had: a record is never dropped for what one of its values does."""

    def render(self, parts: dict[str, Any]) -> str:
        raise NotImplementedError

    def format(self, record: logging.LogRecord) -> str:
        try:
            return self.render(entry(record))
        except Exception as error:  # noqa: BLE001 - whatever a value did, a line is written
            return self.render(fallback(record, error))


class JsonFormatter(_Formatter):
    """One JSON object per record, ASCII only, so no byte of it is a line
    break or a terminal escape."""

    def render(self, parts: dict[str, Any]) -> str:
        return json.dumps(parts, ensure_ascii=True, allow_nan=False, default=_described)


class TextFormatter(_Formatter):
    """The same parts on one readable line: every value is JSON-encoded."""

    def render(self, parts: dict[str, Any]) -> str:
        head = [parts["timestamp"], parts["level"], parts["logger"], parts["event"]]
        tail = [json.dumps(parts["message"], default=_described)]
        fields = parts["fields"].items()
        tail += [f"{key}={json.dumps(value, default=_described)}" for key, value in fields]
        return " ".join(head + tail)


def log_format(env: Mapping[str, str]) -> str:
    """``FORECASTD_LOG_FORMAT``: ``json`` (the default) or ``text``."""
    chosen = (env.get("FORECASTD_LOG_FORMAT") or "json").strip().lower()
    if chosen not in FORMATS:
        raise ValueError(f"FORECASTD_LOG_FORMAT must be json or text, got {chosen!r}")
    return chosen


def log_level(env: Mapping[str, str]) -> int:
    """``FORECASTD_LOG_LEVEL`` as a ``logging`` level; ``INFO`` when unset."""
    chosen = (env.get("FORECASTD_LOG_LEVEL") or "INFO").strip().upper()
    if chosen not in LEVELS:
        raise ValueError(f"FORECASTD_LOG_LEVEL must be one of {', '.join(LEVELS)}, got {chosen!r}")
    return getattr(logging, chosen)


def _process_crashed(kind: type[BaseException], error: BaseException, trace: Any) -> None:
    event(
        logging.getLogger("forecastd"),
        logging.CRITICAL,
        "process.crashed",
        "uncaught exception",
        exc_info=(kind, error, trace),
    )


def _thread_crashed(args: threading.ExceptHookArgs) -> None:
    if args.exc_value is None:
        return
    event(
        logging.getLogger("forecastd"),
        logging.ERROR,
        "thread.crashed",
        "uncaught exception in a thread",
        exc_info=(args.exc_type, args.exc_value, args.exc_traceback),
        thread=args.thread.name if args.thread else "unknown",
    )


class _Stderr(logging.StreamHandler):
    """Writes to whatever ``sys.stderr`` is when the record arrives, not
    to what it was when logging was configured: a stream that was closed
    and put back is written to again."""

    def __init__(self) -> None:
        logging.Handler.__init__(self)

    @property
    def stream(self) -> Any:
        return sys.stderr


def configure(env: Mapping[str, str]) -> None:
    """Send every record of the process through one formatter on stderr:
    this package's, uvicorn's, the ``warnings`` module's and any uncaught
    exception. Raises ``ValueError`` for a setting it does not know, before
    anything is changed."""
    formatter = JsonFormatter() if log_format(env) == "json" else TextFormatter()
    level = log_level(env)
    # A line that cannot be written (stderr closed, a full pipe gone) is
    # lost; ``logging`` would otherwise print a traceback to the same
    # stream, as text, and can fail the caller doing it.
    logging.raiseExceptions = False
    handler = _Stderr()
    handler.setFormatter(formatter)
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(level)
    for name in UVICORN_LOGGERS:
        logger = logging.getLogger(name)
        logger.handlers.clear()
        logger.propagate = True
    logging.captureWarnings(True)
    sys.excepthook = _process_crashed
    threading.excepthook = _thread_crashed
    # A download would otherwise draw its progress bar on stderr.
    os.environ.setdefault("HF_HUB_DISABLE_PROGRESS_BARS", "1")

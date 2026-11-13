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

ExcInfo = tuple[type[BaseException], BaseException, TracebackType | None]


def clip(value: str, limit: int = MAX_STRING) -> str:
    """``value``, cut to ``limit`` characters and marked when it was cut."""
    if len(value) <= limit:
        return value
    return value[:limit] + TRUNCATED


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
        return clip(str(value))
    return _described(value)


def bounded(value: Any, depth: int = 0) -> Any:
    """``value`` as something JSON can hold and a log line can afford:
    strings cut, collections cut, everything else described."""
    if depth < MAX_DEPTH and isinstance(value, Mapping):
        items = list(value.items())[:MAX_ITEMS]
        return {clip(str(key), MAX_KEY): bounded(item, depth + 1) for key, item in items}
    if depth < MAX_DEPTH and isinstance(value, (list, tuple)):
        return [bounded(item, depth + 1) for item in value[:MAX_ITEMS]]
    return _scalar(value)


def error_fields(error: BaseException, stack: str | None = None) -> dict[str, Any]:
    """The fields that describe an exception, named as the daemon names
    them (``error.type``, ``error.message``)."""
    fields: dict[str, Any] = {
        "error.type": type(error).__name__,
        "error.message": clip(str(error)),
    }
    if stack is not None:
        fields["error.stack"] = clip(stack, MAX_STACK)
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
        "event": clip(str(getattr(record, "event", PLAIN_EVENT)), MAX_KEY),
        "message": clip(str(record.msg)),
        "fields": _record_fields(record),
    }


class JsonFormatter(logging.Formatter):
    """One JSON object per record, ASCII only, so no byte of it is a line
    break or a terminal escape."""

    def format(self, record: logging.LogRecord) -> str:
        return json.dumps(entry(record), ensure_ascii=True, allow_nan=False, default=_described)


class TextFormatter(logging.Formatter):
    """The same parts on one readable line: every value is JSON-encoded."""

    def format(self, record: logging.LogRecord) -> str:
        parts = entry(record)
        head = [parts["timestamp"], parts["level"], parts["logger"], parts["event"]]
        tail = [json.dumps(parts["message"])]
        tail += [f"{key}={json.dumps(value)}" for key, value in parts["fields"].items()]
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


def configure(env: Mapping[str, str]) -> None:
    """Send every record of the process through one formatter on stderr:
    this package's, uvicorn's, the ``warnings`` module's and any uncaught
    exception. Raises ``ValueError`` for a setting it does not know, before
    anything is changed."""
    formatter = JsonFormatter() if log_format(env) == "json" else TextFormatter()
    level = log_level(env)
    handler = logging.StreamHandler(sys.stderr)
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

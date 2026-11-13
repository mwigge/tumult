"""Transport-independent request handling.

``handle_forecast`` validates a ``POST /forecast`` payload, runs the model
and shapes the response; ``handle_healthz`` describes the loaded model. Both
return plain dicts so the stdlib server, the FastAPI app and the tests share
one code path.
"""

from __future__ import annotations

import logging
import math
from typing import Any

import numpy as np

from . import __version__
from .model import ForecastModel

log = logging.getLogger("forecastd")

# The daemon mirrors the first two (tumult-api/src/forecast/client.rs) and
# never asks for more.
MAX_CONTEXT = 32_768
MAX_HORIZON = 4_096
# Series forecast together, and covariate rows of either kind.
MAX_VARIATES = 8
MAX_COVARIATES = 8
MAX_LEVELS = 32
# The largest request body either server reads.
MAX_BODY = 8 << 20


class BadRequest(ValueError):
    """The payload is malformed; the message is safe to return."""


def _number(item: Any, name: str) -> float:
    """A JSON number as a finite float; ``null`` is a gap (NaN). Strings,
    booleans and anything non-finite are refused rather than coerced."""
    if item is None:
        return math.nan
    if isinstance(item, bool) or not isinstance(item, (int, float)):
        raise BadRequest(f"{name} must hold numbers or null")
    try:
        number = float(item)
    except OverflowError as error:
        raise BadRequest(f"{name} holds a number too large to represent") from error
    if not math.isfinite(number):
        raise BadRequest(f"{name} must hold finite numbers or null")
    return number


def _as_rows(value: Any, name: str, max_rows: int, width: int | None = None) -> np.ndarray:
    """``[..]`` or ``[[..], ..]`` → float array ``(V, T)`` with NaN gaps."""
    if not isinstance(value, list) or not value:
        raise BadRequest(f"{name} must be a non-empty array")
    rows = value if isinstance(value[0], list) else [value]
    if len(rows) > max_rows:
        raise BadRequest(f"{name} has {len(rows)} rows; at most {max_rows} are accepted")
    if not all(isinstance(row, list) for row in rows):
        raise BadRequest(f"{name} must be one array of numbers or an array of such arrays")
    length = len(rows[0])
    if length == 0 or any(len(row) != length for row in rows):
        raise BadRequest(f"{name} rows must be non-empty and equally long")
    if width is not None and length != width:
        raise BadRequest(f"{name} rows must be {width} long, got {length}")
    if width is None and length > MAX_CONTEXT:
        raise BadRequest(f"{name} is longer than {MAX_CONTEXT} steps")
    return np.array([[_number(item, name) for item in row] for row in rows], dtype=float)


def _nearest_level(levels: tuple[float, ...], wanted: float) -> int:
    return min(range(len(levels)), key=lambda i: abs(levels[i] - wanted))


def parse_levels(raw: Any, available: tuple[float, ...]) -> list[float]:
    if raw is None:
        return list(available)
    if not isinstance(raw, list) or not raw:
        raise BadRequest("quantile_levels must be a non-empty array")
    if len(raw) > MAX_LEVELS:
        raise BadRequest(f"at most {MAX_LEVELS} quantile levels are accepted")
    levels: list[float] = []
    for item in raw:
        if item is None:
            raise BadRequest("quantile_levels must be numbers")
        level = _number(item, "quantile_levels")
        if not 0.0 < level < 1.0:
            raise BadRequest(f"quantile level {level} is outside (0, 1)")
        levels.append(level)
    return levels


def _run_model(
    model: ForecastModel,
    context: np.ndarray,
    horizon: int,
    past: np.ndarray | None,
    known: np.ndarray | None,
) -> np.ndarray:
    """Run the model. A ``ValueError`` is the model turning this request
    down (a shape or length it does not support), so it is the caller's
    error, not a fault of the service."""
    try:
        quantiles = model.forecast(context, horizon, past, known)
    except BadRequest:
        raise
    except ValueError as error:
        log.warning("the model refused a request: %s", error)
        raise BadRequest(f"the model refused the request: {error}") from error
    if quantiles.shape != (context.shape[0], horizon, len(model.levels)):
        raise RuntimeError(f"model returned quantiles shaped {quantiles.shape}")
    return quantiles


def handle_forecast(payload: Any, model: ForecastModel) -> dict[str, Any]:
    """Run one forecast. Raises ``BadRequest`` for malformed input."""
    if not isinstance(payload, dict):
        raise BadRequest("the body must be a JSON object")
    context = _as_rows(payload.get("context"), "context", MAX_VARIATES)
    if not np.isfinite(context).any():
        raise BadRequest("context has no observed points")
    horizon = payload.get("horizon")
    if not isinstance(horizon, int) or isinstance(horizon, bool) or horizon < 1:
        raise BadRequest("horizon must be a positive integer")
    if horizon > MAX_HORIZON:
        raise BadRequest(f"horizon is longer than {MAX_HORIZON} steps")
    past = payload.get("past_covariates")
    known = payload.get("known_covariates")
    steps = context.shape[1]
    past_array = (
        None if past is None else _as_rows(past, "past_covariates", MAX_COVARIATES, steps)
    )
    known_array = (
        None
        if known is None
        else _as_rows(known, "known_covariates", MAX_COVARIATES, steps + horizon)
    )
    wanted = parse_levels(payload.get("quantile_levels"), model.levels)
    quantiles = _run_model(model, context, horizon, past_array, known_array)
    columns = [_nearest_level(model.levels, level) for level in wanted]
    picked = quantiles[:, :, columns]
    series = [
        [[_json_float(v) for v in step] for step in variate.tolist()] for variate in picked
    ]
    return {
        "levels": [model.levels[c] for c in columns],
        "requested_levels": wanted,
        "horizon": horizon,
        "context_points": int(np.isfinite(context).sum(axis=1).max()),
        # One entry per variate; a single-variate request still gets a list
        # of one so the shape never changes under the client.
        "quantiles": series,
        "model": model.info.as_dict(),
        "service_version": __version__,
    }


def _json_float(value: float) -> float | None:
    return float(value) if math.isfinite(value) else None


def handle_healthz(model: ForecastModel) -> dict[str, Any]:
    return {
        "status": "ok",
        "service_version": __version__,
        "levels": list(model.levels),
        "max_context": MAX_CONTEXT,
        "max_horizon": MAX_HORIZON,
        "max_variates": MAX_VARIATES,
        "max_covariates": MAX_COVARIATES,
        "max_body_bytes": MAX_BODY,
        **model.info.as_dict(),
    }

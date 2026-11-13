"""Transport-independent request handling.

``handle_forecast`` validates a ``POST /forecast`` payload, runs the model
and shapes the response; ``handle_healthz`` describes the loaded model. Both
return plain dicts so the stdlib server, the FastAPI app and the tests share
one code path.
"""

from __future__ import annotations

import math
from typing import Any

import numpy as np

from . import __version__
from .model import ForecastModel

MAX_CONTEXT = 32_768
MAX_HORIZON = 4_096


class BadRequest(ValueError):
    """The payload is malformed; the message is safe to return."""


def _as_rows(value: Any, name: str, width: int | None = None) -> np.ndarray:
    """``[..]`` or ``[[..], ..]`` → float array ``(V, T)`` with NaN gaps."""
    if not isinstance(value, list) or not value:
        raise BadRequest(f"{name} must be a non-empty array")
    rows = value if isinstance(value[0], list) else [value]
    try:
        array = np.array(
            [[math.nan if item is None else float(item) for item in row] for row in rows],
            dtype=float,
        )
    except (TypeError, ValueError) as error:
        raise BadRequest(f"{name} must hold numbers or null") from error
    if array.ndim != 2 or array.shape[1] == 0:
        raise BadRequest(f"{name} rows must be non-empty and equally long")
    if width is not None and array.shape[1] != width:
        raise BadRequest(f"{name} rows must be {width} long, got {array.shape[1]}")
    return array


def _nearest_level(levels: tuple[float, ...], wanted: float) -> int:
    return min(range(len(levels)), key=lambda i: abs(levels[i] - wanted))


def parse_levels(raw: Any, available: tuple[float, ...]) -> list[float]:
    if raw is None:
        return list(available)
    if not isinstance(raw, list) or not raw:
        raise BadRequest("quantile_levels must be a non-empty array")
    levels: list[float] = []
    for item in raw:
        try:
            level = float(item)
        except (TypeError, ValueError) as error:
            raise BadRequest("quantile_levels must be numbers") from error
        if not 0.0 < level < 1.0:
            raise BadRequest(f"quantile level {level} is outside (0, 1)")
        levels.append(level)
    return levels


def handle_forecast(payload: Any, model: ForecastModel) -> dict[str, Any]:
    """Run one forecast. Raises ``BadRequest`` for malformed input."""
    if not isinstance(payload, dict):
        raise BadRequest("the body must be a JSON object")
    context = _as_rows(payload.get("context"), "context")
    if context.shape[1] > MAX_CONTEXT:
        raise BadRequest(f"context is longer than {MAX_CONTEXT} steps")
    if not np.isfinite(context).any():
        raise BadRequest("context has no observed points")
    horizon = payload.get("horizon")
    if not isinstance(horizon, int) or isinstance(horizon, bool) or horizon < 1:
        raise BadRequest("horizon must be a positive integer")
    if horizon > MAX_HORIZON:
        raise BadRequest(f"horizon is longer than {MAX_HORIZON} steps")
    past = payload.get("past_covariates")
    known = payload.get("known_covariates")
    past_array = None if past is None else _as_rows(past, "past_covariates", context.shape[1])
    known_array = (
        None
        if known is None
        else _as_rows(known, "known_covariates", context.shape[1] + horizon)
    )
    wanted = parse_levels(payload.get("quantile_levels"), model.levels)
    quantiles = model.forecast(context, horizon, past_array, known_array)
    if quantiles.shape != (context.shape[0], horizon, len(model.levels)):
        raise RuntimeError(f"model returned quantiles shaped {quantiles.shape}")
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
        **model.info.as_dict(),
    }

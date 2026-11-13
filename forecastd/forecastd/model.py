"""Model providers behind the service.

``ForecastModel`` is the seam: ``StubModel`` is a deterministic closed-form
model (level + least-squares trend, normal band from the residual spread)
that needs only numpy and makes the tests and the smoke deployment work;
``TimestralModel`` wraps Mistral's Timestral forecaster and imports it (and
PyTorch) lazily, so nothing here needs torch until a Timestral model is
actually selected.
"""

from __future__ import annotations

import hashlib
import os
from dataclasses import dataclass
from pathlib import Path
from statistics import NormalDist
from typing import Protocol

import numpy as np

# Timestral emits 21 quantiles per step: 0.01, 0.05, 0.10 … 0.90, 0.95, 0.99.
TIMESTRAL_LEVELS: tuple[float, ...] = tuple(
    [0.01, 0.05] + [round(0.1 + 0.05 * i, 2) for i in range(17)] + [0.95, 0.99]
)
assert len(TIMESTRAL_LEVELS) == 21

MODEL_SIZES = ("5M", "28M", "123M", "209M")
DEFAULT_MODEL_SIZE = "5M"


@dataclass(frozen=True)
class ModelInfo:
    """What ``/healthz`` reports and every forecast carries as provenance."""

    model: str
    model_id: str
    size: str
    version: str
    weights_hash: str

    def as_dict(self) -> dict[str, str]:
        return {
            "model": self.model,
            "model_id": self.model_id,
            "size": self.size,
            "version": self.version,
            "weights_hash": self.weights_hash,
        }


class ForecastModel(Protocol):
    """A loaded model: context ``(V, T)`` with NaN gaps to quantiles."""

    info: ModelInfo
    levels: tuple[float, ...]

    def forecast(
        self,
        context: np.ndarray,
        horizon: int,
        past_covariates: np.ndarray | None,
        known_covariates: np.ndarray | None,
    ) -> np.ndarray:
        """Return quantiles shaped ``(V, horizon, len(self.levels))``."""
        ...


def _fill_gaps(row: np.ndarray) -> np.ndarray:
    """Linear interpolation across NaN; edges take the nearest value."""
    finite = np.isfinite(row)
    if not finite.any():
        return np.zeros_like(row)
    idx = np.arange(row.shape[0])
    return np.interp(idx, idx[finite], row[finite])


class StubModel:
    """Deterministic level-plus-trend model with a normal residual band."""

    def __init__(self, levels: tuple[float, ...] = TIMESTRAL_LEVELS) -> None:
        self.levels = levels
        self.info = ModelInfo(
            model="stub",
            model_id="stub-linear",
            size="0",
            version="1",
            weights_hash="none",
        )
        self._z = np.array([NormalDist().inv_cdf(level) for level in levels])

    def forecast(
        self,
        context: np.ndarray,
        horizon: int,
        past_covariates: np.ndarray | None,
        known_covariates: np.ndarray | None,
    ) -> np.ndarray:
        del past_covariates, known_covariates  # the stub ignores covariates
        out = np.empty((context.shape[0], horizon, len(self.levels)))
        steps = np.arange(1, horizon + 1, dtype=float)
        for v, row in enumerate(context):
            y = _fill_gaps(np.asarray(row, dtype=float))
            n = y.shape[0]
            if n >= 2:
                x = np.arange(n, dtype=float)
                slope, intercept = np.polyfit(x, y, 1)
                residual = y - (slope * x + intercept)
                spread = float(residual.std()) if n > 2 else 0.0
                point = intercept + slope * (n - 1 + steps)
            else:
                spread = 0.0
                point = np.full(horizon, float(y[-1]) if n else 0.0)
            widen = np.sqrt(steps)[:, None]
            out[v] = point[:, None] + self._z[None, :] * spread * widen
        return out


def hash_weights(path: str | os.PathLike[str] | None) -> str:
    """SHA-256 over every file under ``path`` (sorted by name), or
    ``unverified`` when the weights come from the hub cache."""
    if path is None:
        return "unverified"
    root = Path(path)
    files = [root] if root.is_file() else sorted(p for p in root.rglob("*") if p.is_file())
    digest = hashlib.sha256()
    for file in files:
        digest.update(file.name.encode())
        with file.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1 << 20), b""):
                digest.update(chunk)
    return digest.hexdigest()


class TimestralModel:
    """Mistral's Timestral forecaster, loaded once.

    ``weights_path`` points at a local snapshot (air-gapped deployments);
    otherwise the model id is fetched through the Hugging Face hub cache.
    The weights carry a modified-MIT licence; they are never bundled in the
    container image — see the README.
    """

    def __init__(
        self,
        size: str = DEFAULT_MODEL_SIZE,
        model_id: str | None = None,
        weights_path: str | None = None,
    ) -> None:
        if size not in MODEL_SIZES:
            raise ValueError(f"unknown Timestral size {size!r}; expected one of {MODEL_SIZES}")
        try:
            from timestral import TimestralForecaster  # noqa: PLC0415 (lazy on purpose)
        except ImportError as error:  # pragma: no cover - depends on the environment
            raise RuntimeError(
                "the timestral package is not installed; "
                "pip install 'git+https://github.com/mistralai/timestral.git' "
                "or run with FORECASTD_MODEL=stub"
            ) from error
        resolved_id = model_id or f"mistralai/Timestral-{size}"
        source = weights_path or resolved_id
        self._forecaster = TimestralForecaster.from_pretrained(source)
        self.levels = TIMESTRAL_LEVELS
        version = getattr(self._forecaster, "version", None) or _package_version("timestral")
        self.info = ModelInfo(
            model="timestral",
            model_id=resolved_id,
            size=size,
            version=str(version),
            weights_hash=hash_weights(weights_path),
        )

    def forecast(
        self,
        context: np.ndarray,
        horizon: int,
        past_covariates: np.ndarray | None,
        known_covariates: np.ndarray | None,
    ) -> np.ndarray:
        quantiles = self._forecaster.forecast(
            context[None, :, :],
            prediction_length=horizon,
            past_covariates=None if past_covariates is None else past_covariates[None],
            known_covariates=None if known_covariates is None else known_covariates[None],
        )
        array = np.asarray(quantiles, dtype=float)
        if array.ndim == 4:
            array = array[0]
        return array


def _package_version(name: str) -> str:
    try:
        from importlib.metadata import version

        return version(name)
    except Exception:  # noqa: BLE001 - a missing distribution is not an error here
        return "unknown"


def load_model_from_env(environ: dict[str, str] | None = None) -> ForecastModel:
    """Pick the model from ``FORECASTD_MODEL`` (``timestral`` by default,
    ``stub`` for tests and smoke runs), ``FORECASTD_MODEL_SIZE`` (5M),
    ``FORECASTD_MODEL_ID`` and ``FORECASTD_WEIGHTS_PATH``."""
    env = os.environ if environ is None else environ
    kind = env.get("FORECASTD_MODEL", "timestral").lower()
    if kind == "stub":
        return StubModel()
    if kind != "timestral":
        raise ValueError(f"unknown FORECASTD_MODEL {kind!r}; expected timestral or stub")
    return TimestralModel(
        size=env.get("FORECASTD_MODEL_SIZE", DEFAULT_MODEL_SIZE),
        model_id=env.get("FORECASTD_MODEL_ID") or None,
        weights_path=env.get("FORECASTD_WEIGHTS_PATH") or None,
    )

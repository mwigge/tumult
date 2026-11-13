"""Model providers behind the service.

``ForecastModel`` is the seam: ``StubModel`` is a deterministic closed-form
model (level + least-squares trend, normal band from the residual spread)
that needs only numpy and makes the tests and the smoke deployment work;
``TimestralModel`` wraps Mistral's Timestral forecaster and imports it (and
PyTorch) lazily, so nothing here needs torch until a Timestral model is
actually selected.

``timestral`` is the default; the stub is an explicit offline mode
(``FORECASTD_MODEL=stub``) and says so in its provenance.
"""

from __future__ import annotations

import hashlib
import os
import threading
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
# The upstream release this service is tested against (not on PyPI).
TIMESTRAL_SOURCE = (
    "git+https://github.com/mistralai/timestral.git@a534bdbd244d64f3d9d743f46c7bf6fc330ed4ff"
)
DEFAULT_MODEL_SIZE = "5M"


@dataclass(frozen=True)
class ModelInfo:
    """What ``/healthz`` reports and every forecast carries as provenance."""

    model: str
    model_id: str
    size: str
    version: str
    weights_hash: str
    # The weights revision: the hub commit of the snapshot, ``local`` for a
    # directory that is not a hub snapshot, ``none`` for the stub.
    revision: str = "none"

    def as_dict(self) -> dict[str, str]:
        return {
            "model": self.model,
            "model_id": self.model_id,
            "size": self.size,
            "version": self.version,
            "revision": self.revision,
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
    ``unverified`` when there is no path to read."""
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


def snapshot_revision(checkpoint: Path) -> str:
    """The hub commit a snapshot directory is named after, else ``local``."""
    if checkpoint.parent.name == "snapshots":
        return checkpoint.name
    return "local"


class TimestralModel:
    """Mistral's Timestral forecaster, loaded once and run on CPU.

    ``weights_path`` points at a local directory holding ``config.json``
    and ``model.safetensors`` (air-gapped deployments); otherwise the model
    id is fetched through the Hugging Face hub cache, at ``revision`` when
    one is pinned. Either way the provenance carries the SHA-256 of the
    files that were loaded. The weights carry a modified-MIT licence; they
    are never bundled in the container image — see the README.
    """

    def __init__(
        self,
        size: str = DEFAULT_MODEL_SIZE,
        model_id: str | None = None,
        weights_path: str | None = None,
        revision: str | None = None,
        threads: int | None = None,
    ) -> None:
        if size not in MODEL_SIZES:
            raise ValueError(f"unknown Timestral size {size!r}; expected one of {MODEL_SIZES}")
        try:
            import torch  # noqa: PLC0415 (lazy on purpose)
            from timestral import TimestralForecaster  # noqa: PLC0415
            from timestral.loading import resolve_checkpoint  # noqa: PLC0415
        except ImportError as error:  # pragma: no cover - depends on the environment
            raise RuntimeError(
                "torch and the timestral package are required for FORECASTD_MODEL=timestral: "
                f"pip install 'timestral @ {TIMESTRAL_SOURCE}' "
                "(or set FORECASTD_MODEL=stub for the offline stand-in)"
            ) from error
        if threads is not None:
            torch.set_num_threads(threads)
        resolved_id = model_id or f"mistralai/Timestral-{size}"
        checkpoint = Path(resolve_checkpoint(weights_path or resolved_id, revision=revision))
        self._forecaster = TimestralForecaster.from_pretrained(checkpoint)
        self._lock = threading.Lock()
        self.levels = tuple(float(level) for level in self._forecaster.quantile_levels)
        self.info = ModelInfo(
            model="timestral",
            model_id=resolved_id,
            size=size,
            version=_package_version("timestral"),
            weights_hash=hash_weights(checkpoint),
            revision=snapshot_revision(checkpoint),
        )

    def forecast(
        self,
        context: np.ndarray,
        horizon: int,
        past_covariates: np.ndarray | None,
        known_covariates: np.ndarray | None,
    ) -> np.ndarray:
        # One forecast at a time: torch already uses every CPU thread it was
        # given, and concurrent calls would only contend.
        with self._lock:
            result = self._forecaster.forecast(
                context[None, :, :],
                prediction_length=horizon,
                past_covariates=None if past_covariates is None else past_covariates[None],
                known_covariates=None if known_covariates is None else known_covariates[None],
            )
        return result.quantiles[0].detach().cpu().numpy().astype(float)


def _package_version(name: str) -> str:
    try:
        from importlib.metadata import version

        return version(name)
    except Exception:  # noqa: BLE001 - a missing distribution is not an error here
        return "unknown"


DEFAULT_THREADS = 4


def default_threads() -> int:
    """Torch threads when ``FORECASTD_THREADS`` is unset: a few cores, not
    all of them. The 5M model gains nothing past four, and claiming every
    core makes each forecast slower on a host that is busy with other work."""
    return max(1, min(DEFAULT_THREADS, os.cpu_count() or 1))


def _optional_int(value: str | None, name: str) -> int | None:
    if value is None or not value.strip():
        return None
    try:
        number = int(value)
    except ValueError as error:
        raise ValueError(f"{name} must be a whole number, got {value!r}") from error
    if number < 1:
        raise ValueError(f"{name} must be at least 1, got {number}")
    return number


def load_model_from_env(environ: dict[str, str] | None = None) -> ForecastModel:
    """Pick the model from ``FORECASTD_MODEL``: ``timestral`` (the default,
    the real model) or ``stub`` (the offline stand-in, no torch and no
    weights). ``FORECASTD_MODEL_SIZE`` (5M), ``FORECASTD_MODEL_ID``,
    ``FORECASTD_MODEL_REVISION``, ``FORECASTD_WEIGHTS_PATH`` and
    ``FORECASTD_THREADS`` (4, or fewer on a smaller host) configure the
    real one."""
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
        revision=env.get("FORECASTD_MODEL_REVISION") or None,
        threads=_optional_int(env.get("FORECASTD_THREADS"), "FORECASTD_THREADS")
        or default_threads(),
    )

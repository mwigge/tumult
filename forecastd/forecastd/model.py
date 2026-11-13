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
import re
import shutil
import tempfile
import threading
from collections.abc import Callable
from dataclasses import dataclass
from importlib import metadata
from pathlib import Path
from statistics import NormalDist
from typing import Any, BinaryIO, Protocol

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
DEFAULT_MODEL_ID = "mistralai/Timestral-5M"
# The weights this service is tested against: the hub commit of
# ``mistralai/Timestral-5M`` and the ``hash_weights`` digest of its files.
# Weights with any other digest are refused before they are loaded.
DEFAULT_MODEL_REVISION = "bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f"
DEFAULT_WEIGHTS_SHA256 = "577236a804ccb3c2b02638c94b6b433920acf8edd248c9f7a6a32eb50104d63e"
_SHA256 = re.compile(r"[0-9a-f]{64}")
# The two files Timestral reads from a checkpoint directory. Only these are
# hashed, so a download that also brought a README or the hub's bookkeeping
# verifies the same as a bare snapshot.
CHECKPOINT_FILES = ("config.json", "model.safetensors")


class WeightsMismatch(RuntimeError):
    """The weights on disk are not the pinned ones; nothing was loaded."""


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


def _weight_files(root: Path, only: tuple[str, ...] | None) -> list[tuple[str, Path]]:
    """The files that are hashed, each with the name it is hashed under
    (its path below ``root``), sorted by that name."""
    if root.is_file():
        return [(root.name, root)]
    if only is None:
        files = (p for p in root.rglob("*") if p.is_file())
        return sorted((p.relative_to(root).as_posix(), p) for p in files)
    missing = [name for name in sorted(only) if not (root / name).is_file()]
    if missing:
        raise FileNotFoundError(f"{root} has no {' or '.join(missing)}")
    return [(name, root / name) for name in sorted(only)]


def _hash_file(digest: Any, name: str, source: Path, sink: BinaryIO | None = None) -> None:
    """Add one file to ``digest``, writing the bytes that were hashed to
    ``sink`` when there is one.

    Each file is framed: the length of its name, the name, the length of
    its content, the content (lengths as 8 bytes, big-endian). Without the
    lengths two different sets of files could share one byte stream, a
    byte moved from the end of one file to the start of the next."""
    encoded = name.encode()
    with source.open("rb") as handle:
        size = os.fstat(handle.fileno()).st_size
        digest.update(len(encoded).to_bytes(8, "big") + encoded + size.to_bytes(8, "big"))
        read = 0
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            read += len(chunk)
            digest.update(chunk)
            if sink is not None:
                sink.write(chunk)
    if read != size:
        raise WeightsMismatch(
            f"{source} changed while it was read: {read} bytes where {size} were announced"
        )


def hash_weights(
    path: str | os.PathLike[str] | None, only: tuple[str, ...] | None = None
) -> str:
    """SHA-256 over every file under ``path`` (sorted by name, framed as
    ``_hash_file`` describes), or ``unverified`` when there is no path to
    read. ``only`` names the files in the directory that count; all of
    them must exist."""
    if path is None:
        return "unverified"
    digest = hashlib.sha256()
    for name, file in _weight_files(Path(path), only):
        _hash_file(digest, name, file)
    return digest.hexdigest()


def _require_pin(actual: str, expected: str, path: str | os.PathLike[str]) -> str:
    if actual != expected:
        raise WeightsMismatch(
            f"the weights at {path} hash to {actual}, not the pinned {expected}; "
            "refusing to load them (set FORECASTD_WEIGHTS_SHA256 only after checking "
            "where the files came from)"
        )
    return actual


def verify_weights(
    path: str | os.PathLike[str], expected: str, only: tuple[str, ...] | None = None
) -> str:
    """Hash the files under ``path`` and return the digest when it is the
    expected one. Raises ``WeightsMismatch`` otherwise. This checks a
    path; to load what was checked, use ``stage_weights``."""
    return _require_pin(hash_weights(path, only), expected, path)


def _copy_hashed(digest: Any, name: str, source: Path, target: Path) -> None:
    """Copy ``source`` to a new file only this user can read, adding the
    bytes written to ``digest``."""
    target.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as sink:
        _hash_file(digest, name, source, sink)
    target.chmod(0o400)


def discard_staged(staged: Path) -> None:
    """Remove a directory ``stage_weights`` made."""
    staged.chmod(0o700)
    shutil.rmtree(staged)


def stage_weights(
    path: str | os.PathLike[str], expected: str, only: tuple[str, ...] | None = None
) -> tuple[Path, str]:
    """Copy the weights under ``path`` into a new private directory and
    return it with its digest, when the digest is the expected one.

    The digest is taken over the bytes as they are written to the copy, and
    the copy (mode 0500, files 0400, in the temporary directory) belongs to
    this process alone. Loading from it loads exactly what was checked:
    replacing the files at ``path`` afterwards changes nothing. On a
    mismatch the copy is removed and ``WeightsMismatch`` is raised. The
    caller removes the directory with ``discard_staged``."""
    staged = Path(tempfile.mkdtemp(prefix="forecastd-weights-"))
    try:
        digest = hashlib.sha256()
        for name, source in _weight_files(Path(path), only):
            _copy_hashed(digest, name, source, staged / name)
        actual = _require_pin(digest.hexdigest(), expected, path)
        staged.chmod(0o500)
    except BaseException:
        discard_staged(staged)
        raise
    return staged, actual


def load_verified(
    checkpoint: Path, expected: str, load: Callable[[Path], Any]
) -> tuple[Any, str]:
    """Call ``load`` on a verified private copy of the checkpoint and
    return what it made with the digest. ``load`` is not called at all
    when the weights are not the expected ones; the copy is removed
    either way."""
    staged, digest = stage_weights(checkpoint, expected, CHECKPOINT_FILES)
    try:
        return load(staged), digest
    finally:
        discard_staged(staged)


@dataclass(frozen=True)
class Backend:
    """What the service takes from torch and timestral, as three calls, so
    the order they are made in can be tested without either installed."""

    # ``resolve(source, revision=...)``: the checkpoint directory of a
    # model id (through the hub cache) or of a local path.
    resolve: Callable[..., str | os.PathLike[str]]
    # ``load(directory)``: the forecaster read from that directory.
    load: Callable[[Path], Any]
    set_threads: Callable[[int], None]


def timestral_backend() -> Backend:
    """The real thing. Imported here, so nothing needs torch until a
    Timestral model is asked for."""
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
    return Backend(
        resolve=resolve_checkpoint,
        load=TimestralForecaster.from_pretrained,
        set_threads=torch.set_num_threads,
    )


def _default_model(env: dict[str, str]) -> bool:
    size = env.get("FORECASTD_MODEL_SIZE") or DEFAULT_MODEL_SIZE
    model_id = env.get("FORECASTD_MODEL_ID") or DEFAULT_MODEL_ID
    return size == DEFAULT_MODEL_SIZE and model_id == DEFAULT_MODEL_ID


def weights_pin(env: dict[str, str]) -> tuple[str | None, str]:
    """The ``(revision, sha256)`` the weights must match.

    The default model is pinned to the tested revision and digest;
    ``FORECASTD_MODEL_REVISION`` and ``FORECASTD_WEIGHTS_SHA256`` override
    either. Any other model has no tested pin and must bring both (the
    digest alone when ``FORECASTD_WEIGHTS_PATH`` mounts the files, since a
    directory has no hub revision — the revision is then ``None``)."""
    revision = env.get("FORECASTD_MODEL_REVISION") or None
    sha = (env.get("FORECASTD_WEIGHTS_SHA256") or "").strip().lower() or None
    mounted = bool(env.get("FORECASTD_WEIGHTS_PATH"))
    if sha is not None and not _SHA256.fullmatch(sha):
        raise ValueError("FORECASTD_WEIGHTS_SHA256 must be 64 hexadecimal characters")
    if _default_model(env):
        revision = revision or DEFAULT_MODEL_REVISION
        sha = sha or DEFAULT_WEIGHTS_SHA256
    if sha is None or (revision is None and not mounted):
        raise ValueError(
            "only the default model has a tested pin: another size or model id needs "
            "FORECASTD_WEIGHTS_SHA256 and FORECASTD_MODEL_REVISION "
            "(the revision is not needed with FORECASTD_WEIGHTS_PATH)"
        )
    return (None if mounted else revision), sha


def snapshot_revision(checkpoint: Path) -> str:
    """The hub commit a snapshot directory is named after, else ``local``."""
    if checkpoint.parent.name == "snapshots":
        return checkpoint.name
    return "local"


class TimestralModel:
    """Mistral's Timestral forecaster, loaded once and run on CPU.

    ``weights_path`` points at a local directory holding ``config.json``
    and ``model.safetensors`` (air-gapped deployments); otherwise the model
    id is fetched through the Hugging Face hub cache at ``revision``.
    Either way the files are copied to a private directory, the copy is
    hashed and compared with ``weights_sha256``, and the forecaster opens
    the copy (``load_verified``); a mismatch raises ``WeightsMismatch`` and
    nothing is loaded. The weights carry a modified-MIT licence; they are
    never bundled in the container image — see the README.
    """

    def __init__(
        self,
        weights_sha256: str,
        size: str = DEFAULT_MODEL_SIZE,
        model_id: str | None = None,
        weights_path: str | None = None,
        revision: str | None = None,
        threads: int | None = None,
        backend: Backend | None = None,
    ) -> None:
        if size not in MODEL_SIZES:
            raise ValueError(f"unknown Timestral size {size!r}; expected one of {MODEL_SIZES}")
        backend = backend or timestral_backend()
        if threads is not None:
            backend.set_threads(threads)
        resolved_id = model_id or f"mistralai/Timestral-{size}"
        checkpoint = Path(backend.resolve(weights_path or resolved_id, revision=revision))
        self._forecaster, weights_hash = load_verified(checkpoint, weights_sha256, backend.load)
        self._lock = threading.Lock()
        self.levels = tuple(float(level) for level in self._forecaster.quantile_levels)
        self.info = ModelInfo(
            model="timestral",
            model_id=resolved_id,
            size=size,
            version=_package_version("timestral"),
            weights_hash=weights_hash,
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
    """The installed version, or ``unknown`` for a package that was put on
    the path without its metadata. Anything else that goes wrong is raised."""
    try:
        return metadata.version(name)
    except metadata.PackageNotFoundError:
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


def load_model_from_env(
    environ: dict[str, str] | None = None, backend: Backend | None = None
) -> ForecastModel:
    """Pick the model from ``FORECASTD_MODEL``: ``timestral`` (the default,
    the real model) or ``stub`` (the offline stand-in, no torch and no
    weights). ``FORECASTD_MODEL_SIZE`` (5M), ``FORECASTD_MODEL_ID``,
    ``FORECASTD_MODEL_REVISION``, ``FORECASTD_WEIGHTS_SHA256``,
    ``FORECASTD_WEIGHTS_PATH`` and ``FORECASTD_THREADS`` (4, or fewer on a
    smaller host) configure the real one; see ``weights_pin`` for which
    weights it will agree to load. ``backend`` stands in for torch and
    timestral in tests."""
    env = os.environ if environ is None else environ
    kind = env.get("FORECASTD_MODEL", "timestral").lower()
    if kind == "stub":
        return StubModel()
    if kind != "timestral":
        raise ValueError(f"unknown FORECASTD_MODEL {kind!r}; expected timestral or stub")
    revision, weights_sha256 = weights_pin(env)
    return TimestralModel(
        weights_sha256=weights_sha256,
        size=env.get("FORECASTD_MODEL_SIZE") or DEFAULT_MODEL_SIZE,
        model_id=env.get("FORECASTD_MODEL_ID") or None,
        weights_path=env.get("FORECASTD_WEIGHTS_PATH") or None,
        revision=revision,
        threads=_optional_int(env.get("FORECASTD_THREADS"), "FORECASTD_THREADS")
        or default_threads(),
        backend=backend,
    )

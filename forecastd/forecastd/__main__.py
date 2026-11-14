"""``python -m forecastd``: serve on ``FORECASTD_BIND`` (default
``0.0.0.0:8700``) with uvicorn when FastAPI is installed, else with the
stdlib server. The model loads once, before the socket opens."""

from __future__ import annotations

import logging
import os
import sys

from . import logs
from .logs import event
from .model import ForecastModel, load_model_from_env
from .service import Gate, gate_from_env

log = logging.getLogger("forecastd")


def parse_bind(bind: str) -> tuple[str, int]:
    host, _, port = bind.rpartition(":")
    if not host or not port.isdigit():
        raise ValueError(f"FORECASTD_BIND must be host:port, got {bind!r}")
    return host, int(port)


def configure_logs() -> None:
    """Set the log format from the environment. A setting that is not
    understood falls back to the defaults before it is raised, so the
    refusal itself is a structured line."""
    try:
        logs.configure(os.environ)
    except ValueError:
        logs.configure({})
        raise


def serve_stdlib(model: ForecastModel, host: str, port: int, gate: Gate) -> None:
    from .server import serve

    event(
        log,
        logging.INFO,
        "server.stdlib",
        "fastapi or uvicorn is not installed; serving with the stdlib server",
        host=host,
        port=port,
    )
    server = serve(model, host, port, gate)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


def main() -> int:
    try:
        configure_logs()
        host, port = parse_bind(os.environ.get("FORECASTD_BIND", "0.0.0.0:8700"))
        gate = gate_from_env(os.environ)
        model = load_model_from_env()
    except Exception:  # noqa: BLE001 - reported, and the process does not start
        # A bad setting, weights that are not the pinned ones, a model that
        # will not load: none of them may leave a half-working service up.
        event(log, logging.ERROR, "startup.failed", "forecastd cannot start", exc_info=True)
        return 1
    event(
        log,
        logging.INFO,
        "model.loaded",
        "model loaded",
        max_concurrent=gate.limit,
        **model.info.as_dict(),
    )
    try:
        import uvicorn

        from .asgi import create_app
    except ImportError:
        serve_stdlib(model, host, port, gate)
        return 0
    # ``log_config=None`` leaves uvicorn's loggers to ``logs.configure``;
    # requests are logged by the application, in the same shape.
    level = logging.getLevelName(logs.log_level(os.environ)).lower()
    uvicorn.run(
        create_app(model, gate),
        host=host,
        port=port,
        log_level=level,
        log_config=None,
        access_log=False,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

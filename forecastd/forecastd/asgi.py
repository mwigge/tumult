"""FastAPI application: ``uvicorn forecastd.asgi:app``.

FastAPI is an optional dependency; importing this module without it raises
``RuntimeError`` with the install hint, and ``python -m forecastd`` falls
back to the stdlib server.
"""

# No ``from __future__ import annotations`` here: FastAPI resolves handler
# annotations at registration, and ``Request`` is imported inside
# ``create_app``. As a string it would not resolve, and FastAPI would read
# ``request`` as a required query parameter.
import json
import logging
from typing import Any

from . import service
from .model import ForecastModel, load_model_from_env
from .service import BadRequest, handle_forecast, handle_healthz

log = logging.getLogger("forecastd")


class _Refused(Exception):
    """A request turned down before the model saw it."""

    def __init__(self, status: int, detail: str) -> None:
        super().__init__(detail)
        self.status = status
        self.detail = detail


def _announced_length(header: str | None) -> int | None:
    """The content-length a client announced, checked against the cap."""
    if header is None:
        return None
    if not (header.isascii() and header.isdigit()):
        raise _Refused(400, "content-length is not a number")
    length = int(header)
    if length > service.MAX_BODY:
        raise _Refused(413, f"the body is larger than {service.MAX_BODY} bytes")
    return length


async def _read_body(request: Any) -> bytes:
    """The request body, never more than ``MAX_BODY`` of it: the header is
    checked first, and the stream is cut off when it runs past the cap."""
    _announced_length(request.headers.get("content-length"))
    body = bytearray()
    async for chunk in request.stream():
        body.extend(chunk)
        if len(body) > service.MAX_BODY:
            raise _Refused(413, f"the body is larger than {service.MAX_BODY} bytes")
    return bytes(body)


def _answer(body: bytes, model: ForecastModel) -> dict[str, Any]:
    """Parse and forecast. Runs on a worker thread: both are CPU work."""
    try:
        payload = json.loads(body)
    except ValueError as error:
        raise _Refused(400, "the body is not valid JSON") from error
    try:
        return handle_forecast(payload, model)
    except BadRequest as error:
        raise _Refused(400, str(error)) from error


def create_app(model: ForecastModel | None = None) -> Any:
    try:
        from fastapi import FastAPI, HTTPException, Request
        from fastapi.concurrency import run_in_threadpool
    except ImportError as error:  # pragma: no cover - depends on the environment
        raise RuntimeError("fastapi is not installed; pip install 'tumult-forecastd[serve]'") from error

    loaded = model or load_model_from_env()
    app = FastAPI(title="tumult-forecastd", docs_url=None, redoc_url=None)

    @app.get("/healthz")
    async def healthz() -> dict[str, Any]:
        return handle_healthz(loaded)

    @app.post("/forecast")
    async def forecast(request: Request) -> dict[str, Any]:
        # The model holds the CPU for the whole forecast; on the event loop
        # it would stall every other request, /healthz included.
        try:
            body = await _read_body(request)
            return await run_in_threadpool(_answer, body, loaded)
        except _Refused as refused:
            raise HTTPException(status_code=refused.status, detail=refused.detail) from refused
        except Exception as error:  # noqa: BLE001 - the model is third-party code
            log.exception("forecast failed")
            raise HTTPException(status_code=500, detail="forecast failed") from error

    return app


def __getattr__(name: str) -> Any:
    # ``forecastd.asgi:app`` for uvicorn, built lazily so importing the
    # module for its factory never loads a model.
    if name == "app":
        return create_app()
    raise AttributeError(name)

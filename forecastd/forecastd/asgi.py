"""FastAPI application: ``uvicorn forecastd.asgi:app``.

FastAPI is an optional dependency; importing this module without it raises
``RuntimeError`` with the install hint, and ``python -m forecastd`` falls
back to the stdlib server.
"""

# No ``from __future__ import annotations`` here: FastAPI resolves handler
# annotations at registration, and ``Request`` is imported inside
# ``create_app``. As a string it would not resolve, and FastAPI would read
# ``request`` as a required query parameter.
import asyncio
import logging
import os
from typing import Any

from . import service
from .logs import event
from .model import ForecastModel, load_model_from_env
from .service import BadRequest, Gate, gate_from_env, handle_forecast, handle_healthz, parse_body

log = logging.getLogger("forecastd")
# How long a request body may take to arrive. A request holds a slot of the
# gate while its body is read, so a body that never ends must not hold one.
BODY_TIMEOUT_S = 30


class AccessLog:
    """ASGI middleware: one ``http.access`` event per response, in the
    shape the stdlib server logs it. uvicorn's own access log is off."""

    def __init__(self, app: Any) -> None:
        self.app = app

    async def __call__(self, scope: Any, receive: Any, send: Any) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return

        async def logged(message: Any) -> None:
            if message["type"] == "http.response.start":
                client = scope.get("client")
                event(
                    log,
                    logging.INFO,
                    "http.access",
                    "request served",
                    client=client[0] if client else None,
                    method=scope.get("method"),
                    path=scope.get("path"),
                    status=message["status"],
                )
            await send(message)

        await self.app(scope, receive, logged)


class _Refused(Exception):
    """A request turned down before the model saw it."""

    def __init__(self, status: int, detail: str, headers: dict[str, str] | None = None) -> None:
        super().__init__(detail)
        self.status = status
        self.detail = detail
        self.headers = headers


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


async def _stream_body(request: Any) -> bytes:
    body = bytearray()
    async for chunk in request.stream():
        body.extend(chunk)
        if len(body) > service.MAX_BODY:
            raise _Refused(413, f"the body is larger than {service.MAX_BODY} bytes")
    return bytes(body)


async def _read_body(request: Any) -> bytes:
    """The request body, never more than ``MAX_BODY`` of it and never
    waited for longer than ``BODY_TIMEOUT_S``: the header is checked first,
    and the stream is cut off when it runs past the cap."""
    _announced_length(request.headers.get("content-length"))
    try:
        return await asyncio.wait_for(_stream_body(request), BODY_TIMEOUT_S)
    except asyncio.TimeoutError as error:
        raise _Refused(408, "the body did not arrive in time") from error


def _answer(body: bytes, model: ForecastModel) -> dict[str, Any]:
    """Parse and forecast. Runs on a worker thread: both are CPU work."""
    try:
        return handle_forecast(parse_body(body), model)
    except BadRequest as error:
        raise _Refused(400, str(error)) from error


def _busy(gate: Gate) -> _Refused:
    event(
        log,
        logging.WARNING,
        "forecast.busy",
        "turned a request away: every slot is taken",
        limit=gate.limit,
    )
    return _Refused(503, service.BUSY, {"retry-after": str(service.RETRY_AFTER_S)})


def create_app(model: ForecastModel | None = None, gate: Gate | None = None) -> Any:
    try:
        from fastapi import FastAPI, HTTPException, Request
        from fastapi.concurrency import run_in_threadpool
    except ImportError as error:  # pragma: no cover - depends on the environment
        raise RuntimeError("fastapi is not installed; pip install 'tumult-forecastd[serve]'") from error

    loaded = model or load_model_from_env()
    slots = gate or gate_from_env(os.environ)
    app = FastAPI(title="tumult-forecastd", docs_url=None, redoc_url=None)
    app.add_middleware(AccessLog)

    @app.get("/healthz")
    async def healthz() -> dict[str, Any]:
        return handle_healthz(loaded)

    @app.post("/forecast")
    async def forecast(request: Request) -> dict[str, Any]:
        # The model holds the CPU for the whole forecast; on the event loop
        # it would stall every other request, /healthz included.
        try:
            return await _gated(request)
        except _Refused as refused:
            raise HTTPException(
                status_code=refused.status, detail=refused.detail, headers=refused.headers
            ) from refused
        except Exception as error:  # noqa: BLE001 - the model is third-party code
            event(log, logging.ERROR, "forecast.failed", "forecast failed", exc_info=True)
            raise HTTPException(status_code=500, detail="forecast failed") from error

    async def _gated(request: Request) -> dict[str, Any]:
        if not slots.enter():
            raise _busy(slots)
        # ``run_in_threadpool`` does not return before its thread is done,
        # even when the request is cancelled, so the slot is given back
        # only when the model has let go of it.
        try:
            body = await _read_body(request)
            return await run_in_threadpool(_answer, body, loaded)
        finally:
            slots.leave()

    return app


def __getattr__(name: str) -> Any:
    # ``forecastd.asgi:app`` for uvicorn, built lazily so importing the
    # module for its factory never loads a model.
    if name == "app":
        return create_app()
    raise AttributeError(name)

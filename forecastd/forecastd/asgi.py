"""FastAPI application: ``uvicorn forecastd.asgi:app``.

FastAPI is an optional dependency; importing this module without it raises
``RuntimeError`` with the install hint, and ``python -m forecastd`` falls
back to the stdlib server.
"""

# No ``from __future__ import annotations`` here: FastAPI resolves handler
# annotations at registration, and ``Request`` is imported inside
# ``create_app``. As a string it would not resolve, and FastAPI would read
# ``request`` as a required query parameter.
import logging
from typing import Any

from .model import ForecastModel, load_model_from_env
from .service import BadRequest, handle_forecast, handle_healthz

log = logging.getLogger("forecastd")


def create_app(model: ForecastModel | None = None) -> Any:
    try:
        from fastapi import FastAPI, HTTPException, Request
    except ImportError as error:  # pragma: no cover - depends on the environment
        raise RuntimeError("fastapi is not installed; pip install 'tumult-forecastd[serve]'") from error

    loaded = model or load_model_from_env()
    app = FastAPI(title="tumult-forecastd", docs_url=None, redoc_url=None)

    @app.get("/healthz")
    async def healthz() -> dict[str, Any]:
        return handle_healthz(loaded)

    @app.post("/forecast")
    async def forecast(request: Request) -> dict[str, Any]:
        try:
            payload = await request.json()
        except ValueError as error:
            raise HTTPException(status_code=400, detail="the body is not valid JSON") from error
        try:
            return handle_forecast(payload, loaded)
        except BadRequest as error:
            raise HTTPException(status_code=400, detail=str(error)) from error
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

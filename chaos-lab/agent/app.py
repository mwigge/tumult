"""Local learning API: fixed experiments and a separately configured read-only tutor."""

import asyncio
import os
import re
import secrets
import time
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal
from urllib.parse import urlsplit

from fastapi import FastAPI, HTTPException, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import FileResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel, ConfigDict, Field, StrictBool

from agent.boundary import BodyLimit
from agent.providers import DEFAULT_MODELS, ProviderConfig, ProviderError, ask_provider

COOKIE = "chaos_lab_session"
SESSION_TTL = 3600
MAX_SESSIONS = 128
MAX_BODY = 32768


@dataclass
class Session:
    """An expiring, memory-only provider credential scoped to one browser."""

    touched: float = field(default_factory=time.monotonic)
    provider: ProviderConfig | None = None
    ask_lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    requests: list[float] = field(default_factory=list)


class Payload(BaseModel):
    model_config = ConfigDict(extra="forbid")


class RunRequest(Payload):
    scenario_id: str = Field(min_length=1, max_length=80, pattern=r"^[a-z0-9-]+$")
    armed: StrictBool = False


class SettingsRequest(Payload):
    provider: Literal["openai", "anthropic", "ollama"]
    model: str = Field(
        min_length=1, max_length=200, pattern=r"^[A-Za-z0-9][A-Za-z0-9._:/-]*$"
    )
    api_key: str = Field(default="", max_length=4096, repr=False)


class AskRequest(Payload):
    q: str = Field(min_length=1, max_length=4000)
    run_id: str | None = Field(default=None, max_length=80)


def public_provider(session: Session) -> dict[str, Any]:
    """Credentials never cross back to the browser, even in validation errors."""
    config = session.provider
    return {
        "configured": config is not None,
        "provider": config.provider if config else "openai",
        "model": config.model if config else DEFAULT_MODELS["openai"],
        "defaults": DEFAULT_MODELS,
    }


def control_token() -> str:
    """Load the automatically provisioned lab control credential."""
    token_file = os.environ.get("LAB_CONTROL_TOKEN_FILE")
    token = (
        Path(token_file).read_text().strip()
        if token_file
        else os.environ.get("LAB_CONTROL_TOKEN", "")
    )
    if len(token) < 32:
        raise RuntimeError(
            "Lab control credential is missing. Start with Docker Compose."
        )
    return token


def create_app(*, engine: Any = None, web_dir: Path | None = None) -> FastAPI:
    """Create an isolated app instance for the desktop lab or contract tests."""
    sessions: dict[str, Session] = {}

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        if engine is None:
            from agent.engine import LabEngine

            app.state.engine = LabEngine(
                data_dir=Path(os.environ.get("LAB_DATA_DIR", "/data")),
                target_url=os.environ.get("LAB_TARGET_URL", "http://target:5000"),
                control_token=control_token(),
                tumult_bin=os.environ.get("TUMULT_BIN", "tumult"),
            )
        else:
            app.state.engine = engine
        await app.state.engine.initialize()
        try:
            yield
        finally:
            await app.state.engine.close()
            sessions.clear()

    app = FastAPI(
        title="Tumult Chaos Lab", lifespan=lifespan, docs_url=None, redoc_url=None
    )

    @app.exception_handler(RequestValidationError)
    async def validation_error(_request: Request, _error: RequestValidationError):
        return JSONResponse(
            {"detail": "Invalid request. Check the fields and try again."},
            status_code=422,
        )

    @app.exception_handler(ProviderError)
    async def provider_error(_request: Request, error: ProviderError):
        return JSONResponse({"detail": str(error)}, status_code=error.status_code)

    @app.middleware("http")
    async def local_boundary(request: Request, call_next):
        host = request.headers.get("host", "")
        try:
            hostname = urlsplit("http://" + host).hostname
        except ValueError:
            hostname = None
        if hostname not in {"localhost", "127.0.0.1", "::1"}:
            return JSONResponse(
                {"detail": "Use localhost to access this lab."}, status_code=400
            )
        if request.url.path == "/api/health" and request.method == "GET":
            return await call_next(request)
        if request.method in {"POST", "PUT", "PATCH", "DELETE"}:
            origin = request.headers.get("origin")
            if request.headers.get("sec-fetch-site") == "cross-site" or (
                origin and origin != f"{request.url.scheme}://{host}"
            ):
                return JSONResponse(
                    {"detail": "Cross-origin requests are not allowed."},
                    status_code=403,
                )
            try:
                size = int(request.headers.get("content-length", "0"))
            except ValueError:
                return JSONResponse(
                    {"detail": "Invalid request size."}, status_code=400
                )
            if size < 0 or size > MAX_BODY:
                return JSONResponse(
                    {"detail": "Request is too large."}, status_code=413
                )
            if request.method != "DELETE":
                if "content-length" not in request.headers:
                    return JSONResponse(
                        {"detail": "Content-Length is required."}, status_code=411
                    )
                if (
                    request.headers.get("content-type", "").split(";")[0]
                    != "application/json"
                ):
                    return JSONResponse(
                        {"detail": "Send an application/json request."}, status_code=415
                    )
        now = time.monotonic()
        for key in list(sessions):
            if now - sessions[key].touched > SESSION_TTL:
                del sessions[key]
        sid = request.cookies.get(COOKIE, "")
        current = sessions.get(sid)
        new_session = False
        if current is None:
            if request.method not in {"GET", "HEAD", "OPTIONS"}:
                return JSONResponse(
                    {"detail": "Open or reload the lab before continuing."},
                    status_code=403,
                )
            if len(sessions) >= MAX_SESSIONS:
                return JSONResponse(
                    {"detail": "Too many local sessions. Try again later."},
                    status_code=503,
                )
            sid = secrets.token_urlsafe(32)
            current = Session()
            sessions[sid] = current
            new_session = True
        current.touched = now
        request.state.session = current
        response = await call_next(request)
        response.headers.update(
            {
                "Cache-Control": "no-store",
                "X-Content-Type-Options": "nosniff",
                "Referrer-Policy": "no-referrer",
                "Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
                "Permissions-Policy": "camera=(), microphone=(), geolocation=()",
            }
        )
        if new_session:
            response.set_cookie(
                COOKIE,
                sid,
                httponly=True,
                samesite="strict",
                max_age=SESSION_TTL,
                secure=request.url.scheme == "https",
            )
        return response

    async def find_run(run_id: str) -> dict:
        if not re.fullmatch(r"[A-Za-z0-9-]{1,80}", run_id):
            raise HTTPException(404, "Run not found.")
        try:
            run = await app.state.engine.get(run_id)
        except (ValueError, KeyError):
            raise HTTPException(404, "Run not found.") from None
        if run is None:
            raise HTTPException(404, "Run not found.")
        return run

    @app.get("/api/health")
    async def health():
        return {"status": "ok"}

    @app.get("/api/status")
    async def status(request: Request):
        state = await app.state.engine.health()
        return {
            "engine": {
                "available": state.get("tumult", state.get("available", False)),
                "version": state.get("version"),
            },
            "target": {"healthy": bool(state.get("target", False))},
            "active_run_id": state.get("active_run_id"),
            "provider": public_provider(request.state.session),
            "mode": "local-lab",
        }

    @app.get("/api/scenarios")
    async def scenarios():
        return app.state.engine.scenarios()

    @app.get("/api/runs")
    async def runs():
        return await app.state.engine.list_runs()

    @app.post("/api/runs", status_code=202)
    async def start_run(body: RunRequest):
        if not body.armed:
            raise HTTPException(403, "Arm the lab to run the selected experiment.")
        try:
            return await app.state.engine.start(body.scenario_id)
        except ValueError:
            raise HTTPException(
                400, "Choose a scenario from the lesson catalog."
            ) from None
        except RuntimeError:
            raise HTTPException(
                409,
                "The engine is unavailable or another experiment is active. Check lab status.",
            ) from None

    @app.get("/api/runs/{run_id}")
    async def run_detail(run_id: str):
        return await find_run(run_id)

    @app.post("/api/runs/{run_id}/stop")
    async def stop_run(run_id: str):
        await find_run(run_id)
        return await app.state.engine.stop(run_id)

    @app.get("/api/runs/{run_id}/export")
    async def export_run(run_id: str):
        run = await find_run(run_id)
        return JSONResponse(
            {"format": "tumult-chaos-lab/v1", "run": run},
            headers={
                "Content-Disposition": f'attachment; filename="chaos-lab-{run_id}.json"'
            },
        )

    @app.get("/api/runs/{run_id}/journal")
    async def journal(run_id: str):
        await find_run(run_id)
        path = app.state.engine.journal_path(run_id)
        if path is None or not path.is_file():
            raise HTTPException(404, "The Tumult journal is not available yet.")
        return FileResponse(
            path, filename=f"tumult-{run_id}.toon", media_type="text/plain"
        )

    @app.post("/api/settings")
    async def settings(body: SettingsRequest, request: Request):
        key = body.api_key.strip()
        if any(not 33 <= ord(char) <= 126 for char in key):
            raise HTTPException(400, "Enter a valid API key.")
        if body.provider != "ollama" and not key:
            raise HTTPException(400, "An API key is required for this provider.")
        request.state.session.provider = ProviderConfig(body.provider, body.model, key)
        return public_provider(request.state.session)

    @app.delete("/api/settings")
    async def forget_settings(request: Request):
        request.state.session.provider = None
        return public_provider(request.state.session)

    @app.post("/api/ask")
    async def ask(body: AskRequest, request: Request):
        session = request.state.session
        config = session.provider
        if config is None:
            raise HTTPException(
                409,
                "Connect an AI provider in Tutor settings first. Guided labs work without a key.",
            )
        if not body.q.strip():
            raise HTTPException(400, "Enter a question for the tutor.")
        if session.ask_lock.locked():
            raise HTTPException(429, "Wait for the current tutor response.")
        now = time.monotonic()
        session.requests = [at for at in session.requests if now - at < 60]
        if len(session.requests) >= 10:
            raise HTTPException(
                429, "Tutor limit reached. Wait a minute before asking again."
            )
        async with session.ask_lock:
            session.requests.append(now)
            context = {
                "mode": "isolated application-level fault simulation",
                "scenarios": app.state.engine.scenarios(),
            }
            if body.run_id:
                context["run"] = await find_run(body.run_id)
            answer = await ask_provider(config, body.q, context)
        return {"answer": answer, "provider": config.provider, "model": config.model}

    @app.get("/api/runs/{run_id}/experiment")
    async def experiment(run_id: str):
        await find_run(run_id)
        path = app.state.engine.experiment_path(run_id)
        if path is None or not path.is_file():
            raise HTTPException(404, "The experiment definition is not available yet.")
        return FileResponse(
            path, filename=f"experiment-{run_id}.toon", media_type="text/plain"
        )

    app.add_middleware(BodyLimit, limit=MAX_BODY)
    static_dir = web_dir or Path(__file__).resolve().parent.parent / "web"
    if static_dir.is_dir():
        app.mount("/", StaticFiles(directory=static_dir, html=True), name="web")
    return app


app = create_app()

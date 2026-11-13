"""Zero-dependency HTTP server (stdlib ``http.server``).

Used by the tests and by air-gapped hosts that would rather not install an
ASGI stack. The FastAPI app in ``asgi.py`` serves the same two routes.
"""

from __future__ import annotations

import json
import logging
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from .model import ForecastModel
from .service import BadRequest, handle_forecast, handle_healthz

log = logging.getLogger("forecastd")
MAX_BODY = 64 << 20


def make_handler(model: ForecastModel) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        server_version = "tumult-forecastd"

        def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
            log.info("%s " + format, self.address_string(), *args)

        def _send(self, status: HTTPStatus, body: dict[str, Any]) -> None:
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self) -> None:  # noqa: N802 - http.server naming
            if self.path.split("?", 1)[0] == "/healthz":
                self._send(HTTPStatus.OK, handle_healthz(model))
                return
            self._send(HTTPStatus.NOT_FOUND, {"error": "not found"})

        def do_POST(self) -> None:  # noqa: N802 - http.server naming
            if self.path.split("?", 1)[0] != "/forecast":
                self._send(HTTPStatus.NOT_FOUND, {"error": "not found"})
                return
            length = int(self.headers.get("content-length") or 0)
            if length <= 0 or length > MAX_BODY:
                self._send(HTTPStatus.BAD_REQUEST, {"error": "body is empty or too large"})
                return
            try:
                payload = json.loads(self.rfile.read(length))
            except ValueError:
                self._send(HTTPStatus.BAD_REQUEST, {"error": "body is not JSON"})
                return
            try:
                self._send(HTTPStatus.OK, handle_forecast(payload, model))
            except BadRequest as error:
                self._send(HTTPStatus.BAD_REQUEST, {"error": str(error)})
            except Exception:  # noqa: BLE001 - the model is third-party code
                log.exception("forecast failed")
                self._send(HTTPStatus.INTERNAL_SERVER_ERROR, {"error": "forecast failed"})

    return Handler


def serve(model: ForecastModel, host: str, port: int) -> ThreadingHTTPServer:
    """Bind and return the server; the caller runs ``serve_forever``."""
    server = ThreadingHTTPServer((host, port), make_handler(model))
    server.daemon_threads = True
    return server

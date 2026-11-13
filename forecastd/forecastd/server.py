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

from . import service
from .model import ForecastModel
from .service import BadRequest, handle_forecast, handle_healthz

log = logging.getLogger("forecastd")
# How long a connection may sit without sending or taking bytes. The
# forecast itself is not socket time and is not bounded by this.
REQUEST_TIMEOUT_S = 30


def _content_length(header: str | None) -> int | None:
    """The announced body length, or ``None`` when it is not a plain
    non-negative number."""
    if header is None or not (header.isascii() and header.isdigit()):
        return None
    return int(header)


def make_handler(model: ForecastModel) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        server_version = "tumult-forecastd"
        timeout = REQUEST_TIMEOUT_S

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
            length = _content_length(self.headers.get("content-length"))
            if not length:
                self._send(HTTPStatus.BAD_REQUEST, {"error": "content-length must be positive"})
                return
            if length > service.MAX_BODY:
                # Answered without reading the body; the connection closes.
                self.close_connection = True
                self._send(
                    HTTPStatus.REQUEST_ENTITY_TOO_LARGE,
                    {"error": f"body is larger than {service.MAX_BODY} bytes"},
                )
                return
            try:
                raw = self.rfile.read(length)
            except TimeoutError:
                log.warning("%s sent no body within %d s", self.address_string(), REQUEST_TIMEOUT_S)
                self.close_connection = True
                return
            try:
                payload = json.loads(raw)
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

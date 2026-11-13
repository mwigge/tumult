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
from .logs import event
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


def _host(address: Any) -> str:
    """The host of a socket address, whatever family it is."""
    if isinstance(address, tuple) and address:
        return str(address[0])
    return str(address)


def make_handler(model: ForecastModel) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        server_version = "tumult-forecastd"
        timeout = REQUEST_TIMEOUT_S

        def log_request(self, code: int | str = "-", size: int | str = "-") -> None:
            del size
            event(
                log,
                logging.INFO,
                "http.access",
                "request served",
                client=_host(self.client_address),
                method=self.command,
                path=getattr(self, "path", None),
                status=int(code) if isinstance(code, int) else str(code),
            )

        def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
            # What ``http.server`` itself has to say (a request line it
            # could not parse, a timeout). Its template stays a field.
            event(
                log,
                logging.INFO,
                "http.message",
                "http server message",
                client=_host(self.client_address),
                template=format,
                values=[str(arg) for arg in args],
            )

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
                event(
                    log,
                    logging.WARNING,
                    "http.body_timeout",
                    "the request body did not arrive in time",
                    client=_host(self.client_address),
                    timeout_s=REQUEST_TIMEOUT_S,
                )
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
                event(log, logging.ERROR, "forecast.failed", "forecast failed", exc_info=True)
                self._send(HTTPStatus.INTERNAL_SERVER_ERROR, {"error": "forecast failed"})

    return Handler


class Server(ThreadingHTTPServer):
    """The threaded server, with a handler that fails logged as an event
    (``socketserver`` would print the traceback)."""

    daemon_threads = True

    def handle_error(self, request: Any, client_address: Any) -> None:
        del request
        event(
            log,
            logging.ERROR,
            "http.handler_failed",
            "a request handler failed",
            exc_info=True,
            client=_host(client_address),
        )


def serve(model: ForecastModel, host: str, port: int) -> Server:
    """Bind and return the server; the caller runs ``serve_forever``."""
    return Server((host, port), make_handler(model))

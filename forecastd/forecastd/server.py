"""Zero-dependency HTTP server (stdlib ``http.server``).

Used by the tests and by air-gapped hosts that would rather not install an
ASGI stack. The FastAPI app in ``asgi.py`` serves the same two routes.
"""

from __future__ import annotations

import json
import logging
import time
from collections.abc import Callable
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from . import service
from .logs import event
from .model import ForecastModel
from .service import BadRequest, Gate, handle_forecast, handle_healthz, parse_body

log = logging.getLogger("forecastd")
# How long a connection may sit without sending or taking bytes. The
# forecast itself is not socket time and is not bounded by this.
REQUEST_TIMEOUT_S = 30
# How long a whole request body may take to arrive, however steadily its
# bytes come.
BODY_DEADLINE_S = 30
_CHUNK = 64 << 10


class _BodyLate(Exception):
    """The body deadline passed with ``received`` bytes of it in."""

    def __init__(self, received: int) -> None:
        super().__init__("the body did not arrive in time")
        self.received = received


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


def make_handler(
    model: ForecastModel,
    gate: Gate | None = None,
    clock: Callable[[], float] = time.monotonic,
) -> type[BaseHTTPRequestHandler]:
    """The request handler. ``clock`` is what the body deadline is measured
    with (seconds, monotonic)."""
    gate = gate or Gate()

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

        def _send(
            self, status: HTTPStatus, body: dict[str, Any], retry_after: int | None = None
        ) -> None:
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            if retry_after is not None:
                self.send_header("retry-after", str(retry_after))
            self.end_headers()
            self.wfile.write(data)

        def _read_body(self, length: int, keep: bool) -> bytes:
            """Up to ``length`` bytes of body, a piece at a time, all of it
            within ``BODY_DEADLINE_S``: a client that sends a byte now and
            then stays inside the idle timeout and still runs out of time.
            Raises ``_BodyLate`` at the deadline. ``keep`` false drops the
            pieces."""
            deadline = clock() + BODY_DEADLINE_S
            pieces: list[bytes] = []
            left = length
            try:
                while left > 0:
                    piece = self._read_piece(left, deadline - clock(), length - left)
                    if not piece:
                        break
                    left -= len(piece)
                    if keep:
                        pieces.append(piece)
            finally:
                self.connection.settimeout(REQUEST_TIMEOUT_S)
            return b"".join(pieces)

        def _read_piece(self, left: int, remaining: float, received: int) -> bytes:
            """What has arrived of the body, waiting for no longer than the
            deadline allows."""
            if remaining <= 0:
                raise _BodyLate(received)
            self.connection.settimeout(min(remaining, REQUEST_TIMEOUT_S))
            try:
                return self.rfile.read1(min(left, _CHUNK))
            except TimeoutError as error:
                raise _BodyLate(received) from error

        def _discard_body(self) -> None:
            """Read and drop a body nobody will look at, so the answer is
            not lost to a connection reset. A body with no usable length,
            one over the cap, or one that does not arrive in time is left
            and the connection closes."""
            left = _content_length(self.headers.get("content-length"))
            if left is None or left > service.MAX_BODY:
                self.close_connection = True
                return
            try:
                self._read_body(left, keep=False)
            except _BodyLate:
                self.close_connection = True

        def _busy(self) -> None:
            event(
                log,
                logging.WARNING,
                "forecast.busy",
                "turned a request away: every slot is taken",
                limit=gate.limit,
            )
            self._discard_body()
            self._send(
                HTTPStatus.SERVICE_UNAVAILABLE,
                {"error": service.BUSY},
                retry_after=service.RETRY_AFTER_S,
            )

        def _late(self, length: int, received: int) -> None:
            event(
                log,
                logging.WARNING,
                "http.body_timeout",
                "the request body did not arrive in time",
                client=_host(self.client_address),
                deadline_s=BODY_DEADLINE_S,
                announced=length,
                received=received,
            )
            self.close_connection = True
            self._send(HTTPStatus.REQUEST_TIMEOUT, {"error": "the body did not arrive in time"})

        def do_GET(self) -> None:  # noqa: N802 - http.server naming
            if self.path.split("?", 1)[0] == "/healthz":
                self._send(HTTPStatus.OK, handle_healthz(model))
                return
            self._send(HTTPStatus.NOT_FOUND, {"error": "not found"})

        def do_POST(self) -> None:  # noqa: N802 - http.server naming
            if self.path.split("?", 1)[0] != "/forecast":
                self._send(HTTPStatus.NOT_FOUND, {"error": "not found"})
                return
            if not gate.enter():
                self._busy()
                return
            try:
                self._forecast()
            finally:
                gate.leave()

        def _forecast(self) -> None:
            """Read, parse and answer one forecast request. The caller
            holds a slot of the gate for as long as this runs."""
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
                raw = self._read_body(length, keep=True)
            except _BodyLate as late:
                self._late(length, late.received)
                return
            try:
                self._send(HTTPStatus.OK, handle_forecast(parse_body(raw), model))
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

    def __init__(
        self,
        address: tuple[str, int],
        model: ForecastModel,
        gate: Gate | None,
        clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self.gate = gate or Gate()
        super().__init__(address, make_handler(model, self.gate, clock))

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


def serve(
    model: ForecastModel,
    host: str,
    port: int,
    gate: Gate | None = None,
    clock: Callable[[], float] = time.monotonic,
) -> Server:
    """Bind and return the server; the caller runs ``serve_forever``.
    ``gate`` bounds the forecast requests in hand (four when not given)."""
    return Server((host, port), model, gate, clock)

"""Isolated teaching target. Faults affect only this process's /work endpoint."""

import hmac
import json
import logging
import math
import os
import threading
import time
from collections.abc import Callable
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

FAULTS = frozenset({"latency", "http-errors", "cache-outage", "db-timeout"})
MAX_DURATION_S = 15
LOGGER = logging.getLogger(__name__)


class FaultState:
    """A bounded fault lease; reset wakes requests waiting on simulated latency."""

    def __init__(self, clock: Callable[[], float] = time.monotonic) -> None:
        self._clock = clock
        self._condition = threading.Condition()
        self._fault: str | None = None
        self._deadline = 0.0
        self._generation = 0

    def apply(self, fault: str, duration_s: float) -> None:
        if fault not in FAULTS:
            raise ValueError("Unknown fault")
        if (
            isinstance(duration_s, bool)
            or not isinstance(duration_s, (int, float))
            or not math.isfinite(duration_s)
            or not 0 < duration_s <= MAX_DURATION_S
        ):
            raise ValueError(
                "Fault duration must be greater than zero and at most 15 seconds"
            )
        with self._condition:
            self._fault = fault
            self._deadline = self._clock() + duration_s
            self._generation += 1
            self._condition.notify_all()

    def current(self) -> str | None:
        with self._condition:
            return self._fault if self._clock() < self._deadline else None

    def reset(self) -> None:
        with self._condition:
            self._fault = None
            self._deadline = 0
            self._generation += 1
            self._condition.notify_all()

    def work(self) -> tuple[int, dict]:
        started = self._clock()
        with self._condition:
            fault = self.current()
            generation = self._generation
            delay = {"latency": 0.4, "db-timeout": 0.6}.get(fault or "", 0)
            deadline = min(started + delay, self._deadline)
            while delay and self._clock() < deadline and generation == self._generation:
                self._condition.wait(timeout=deadline - self._clock())
            fault = self.current() if generation == self._generation else None
        status = 503 if fault in {"http-errors", "cache-outage", "db-timeout"} else 200
        return status, {
            "status": "ok" if status == 200 else "degraded",
            "fault": fault,
            "latency_ms": round((self._clock() - started) * 1000, 2),
            "dependencies": {
                "cache": (
                    "simulated-unavailable"
                    if fault == "cache-outage"
                    else "simulated-ok"
                ),
                "database": (
                    "simulated-timeout" if fault == "db-timeout" else "simulated-ok"
                ),
            },
            "simulation": True,
        }


def make_server(host: str, port: int, token: str) -> ThreadingHTTPServer:
    """Create the target server with a required control token."""
    if len(token) < 16:
        raise ValueError("LAB_CONTROL_TOKEN must contain at least 16 characters")
    state = FaultState()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args: object) -> None:
            LOGGER.debug("target_request %s", format % args)

        def reply(self, status: int, payload: dict) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                LOGGER.debug("Target client disconnected")

        def do_GET(self) -> None:
            if self.path == "/health":
                self.reply(
                    200, {"status": "ok", "simulation": True, "fault": state.current()}
                )
                return
            if self.path == "/work":
                self.reply(*state.work())
                return
            self.reply(404, {"error": "Not found"})

        def do_POST(self) -> None:
            if not hmac.compare_digest(
                self.headers.get("Authorization", ""), "Bearer " + token
            ):
                self.reply(401, {"error": "Control authentication required"})
                return
            if self.path == "/reset":
                state.reset()
                self.reply(200, {"status": "reset"})
                return
            if self.path != "/fault":
                self.reply(404, {"error": "Not found"})
                return
            try:
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 < length <= 1024:
                    raise ValueError("Invalid request size")
                self.connection.settimeout(2)
                payload = json.loads(self.rfile.read(length))
                if not isinstance(payload, dict) or set(payload) != {
                    "fault",
                    "duration_s",
                }:
                    raise ValueError("Expected fault and duration_s")
                state.apply(payload["fault"], payload["duration_s"])
            except (ValueError, TypeError, OSError) as exc:
                self.reply(400, {"error": str(exc)})
                return
            self.reply(
                200,
                {
                    "status": "injected",
                    "fault": state.current(),
                    "max_duration_s": MAX_DURATION_S,
                },
            )

    server = ThreadingHTTPServer((host, port), Handler)
    server.daemon_threads = True
    return server


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    make_server(
        # This target is only on the isolated Compose network, with no published port.
        "0.0.0.0",  # nosec B104
        int(os.environ.get("PORT", "5000")),
        (
            os.environ.get("LAB_CONTROL_TOKEN")
            or Path(os.environ["LAB_CONTROL_TOKEN_FILE"]).read_text().strip()
        ),
    ).serve_forever()

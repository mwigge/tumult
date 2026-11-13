#!/usr/bin/env python3
"""The proof's checkout service: answers every GET with 200 after a delay.
A "new build" of the service is this server restarted with other
arguments, so a slower build is really slower.

    target.py <port> <delay_ms> [<slow_every> <slow_ms>]

With the optional pair, every <slow_every>-th request takes <slow_ms>
instead (a slow tail). A <slow_ms> of 0 means "never answer": the
request hangs until the client gives up.

GET /ready answers at once and is neither counted nor traced, like a
health endpoint left out of tracing.

With PROOF_TELEMETRY_URL (the daemon's base URL) and
PROOF_SERVICE_VERSION set, the service reports its own telemetry: a
server span for the requests it serves, at most one every 200 ms, as
service "checkout" in environment PROOF_SERVICE_ENV ("demo" when unset).
A build nobody calls is therefore silent until the load starts.
"""
import itertools
import os
import sys
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from otlp_span import encode

PORT = int(sys.argv[1])
DELAY_S = float(sys.argv[2]) / 1000.0
SLOW_EVERY = int(sys.argv[3]) if len(sys.argv) > 4 else 0
SLOW_S = float(sys.argv[4]) / 1000.0 if len(sys.argv) > 4 else 0.0
COUNTER = itertools.count(1)
NEVER = threading.Event()
TELEMETRY_URL = os.environ.get("PROOF_TELEMETRY_URL", "")
VERSION = os.environ.get("PROOF_SERVICE_VERSION", "")
ENVIRONMENT = os.environ.get("PROOF_SERVICE_ENV", "demo")
SPAN_EVERY_S = 0.2
SERVED = threading.Event()


def report_spans():
    """Export one span for each 200 ms in which a request was served."""
    while True:
        SERVED.wait()
        SERVED.clear()
        request = urllib.request.Request(
            TELEMETRY_URL + "/v1/traces",
            data=encode("checkout", VERSION, ENVIRONMENT),
            headers={"content-type": "application/x-protobuf"},
        )
        try:
            urllib.request.urlopen(request, timeout=5).close()
        except OSError as error:
            print(f"span export failed: {error}", file=sys.stderr, flush=True)
        time.sleep(SPAN_EVERY_S)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):  # noqa: N802 (http.server's naming)
        if self.path == "/ready":
            self.answer()
            return
        slow = SLOW_EVERY > 0 and next(COUNTER) % SLOW_EVERY == 0
        if slow and SLOW_S == 0.0:
            NEVER.wait()
            return
        time.sleep(SLOW_S if slow else DELAY_S)
        self.answer()
        SERVED.set()

    def answer(self):
        body = b"ok\n"
        self.send_response(200)
        self.send_header("content-type", "text/plain")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


class Server(ThreadingHTTPServer):
    daemon_threads = True


if TELEMETRY_URL and VERSION:
    threading.Thread(target=report_spans, daemon=True).start()
Server(("127.0.0.1", PORT), Handler).serve_forever()

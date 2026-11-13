#!/usr/bin/env python3
"""The proof's checkout service: answers every GET with 200 after a delay.
A "new build" of the service is this server restarted with other
arguments, so a slower build is really slower.

    target.py <port> <delay_ms> [<slow_every> <slow_ms>]

With the optional pair, every <slow_every>-th request takes <slow_ms>
instead (a slow tail). A <slow_ms> of 0 means "never answer": the
request hangs until the client gives up.
"""
import itertools
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
DELAY_S = float(sys.argv[2]) / 1000.0
SLOW_EVERY = int(sys.argv[3]) if len(sys.argv) > 4 else 0
SLOW_S = float(sys.argv[4]) / 1000.0 if len(sys.argv) > 4 else 0.0
COUNTER = itertools.count(1)
NEVER = threading.Event()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):  # noqa: N802 (http.server's naming)
        slow = SLOW_EVERY > 0 and next(COUNTER) % SLOW_EVERY == 0
        if slow and SLOW_S == 0.0:
            NEVER.wait()
            return
        time.sleep(SLOW_S if slow else DELAY_S)
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


Server(("127.0.0.1", PORT), Handler).serve_forever()

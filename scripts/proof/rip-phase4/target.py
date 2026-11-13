#!/usr/bin/env python3
"""The proof's checkout service: answers every GET with 200 after a fixed
delay. A "new build" of the service is this server restarted with another
delay, so a slower build is really slower.

    target.py <port> <delay_ms>
"""
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
DELAY_S = float(sys.argv[2]) / 1000.0


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):  # noqa: N802 (http.server's naming)
        time.sleep(DELAY_S)
        body = b"ok\n"
        self.send_response(200)
        self.send_header("content-type", "text/plain")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()

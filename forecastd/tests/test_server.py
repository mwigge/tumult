"""The stdlib server end to end over a loopback socket."""

from __future__ import annotations

import json
import threading
import unittest
import urllib.error
import urllib.request

from forecastd import service
from forecastd.__main__ import parse_bind
from forecastd.model import StubModel
from forecastd.server import REQUEST_TIMEOUT_S, make_handler, serve


class ServerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.server = serve(StubModel(), "127.0.0.1", 0)
        cls.base = f"http://127.0.0.1:{cls.server.server_address[1]}"
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls) -> None:
        cls.server.shutdown()
        cls.server.server_close()

    def _request(self, method: str, path: str, body: bytes | None = None) -> tuple[int, dict]:
        request = urllib.request.Request(
            self.base + path,
            data=body,
            method=method,
            headers={"content-type": "application/json"} if body else {},
        )
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, json.loads(response.read())
        except urllib.error.HTTPError as error:
            with error:
                return error.code, json.loads(error.read())

    def test_healthz(self) -> None:
        status, body = self._request("GET", "/healthz")
        self.assertEqual(status, 200)
        self.assertEqual(body["model"], "stub")

    def test_forecast_round_trip(self) -> None:
        payload = {"context": [1.0, 2.0, 3.0, 4.0], "horizon": 2, "quantile_levels": [0.1, 0.5, 0.9]}
        status, body = self._request("POST", "/forecast", json.dumps(payload).encode())
        self.assertEqual(status, 200)
        self.assertEqual(body["levels"], [0.1, 0.5, 0.9])
        self.assertEqual(len(body["quantiles"][0]), 2)
        self.assertAlmostEqual(body["quantiles"][0][0][1], 5.0, places=6)

    def test_errors_are_json(self) -> None:
        status, body = self._request("POST", "/forecast", b"not json")
        self.assertEqual(status, 400)
        self.assertIn("error", body)
        status, body = self._request("POST", "/forecast", b'{"context": [], "horizon": 1}')
        self.assertEqual(status, 400)
        status, body = self._request("GET", "/nope")
        self.assertEqual(status, 404)
        status, body = self._request("POST", "/nope", b"{}")
        self.assertEqual(status, 404)


    def _raw(self, request: bytes) -> bytes:
        """Send bytes as they are; return everything the server answers."""
        import socket

        with socket.create_connection(self.server.server_address, timeout=5) as sock:
            sock.sendall(request)
            chunks = []
            while chunk := sock.recv(65536):
                chunks.append(chunk)
        return b"".join(chunks)

    def test_a_content_length_that_is_not_a_number_is_400(self) -> None:
        for value in (b"abc", b"-5", b"1e3", b""):
            head = b"POST /forecast HTTP/1.1\r\nHost: x\r\nConnection: close\r\n"
            # No body is sent: nothing is left unread when the server closes.
            answer = self._raw(head + b"Content-Length: " + value + b"\r\n\r\n")
            with self.subTest(value=value):
                self.assertTrue(answer.startswith(b"HTTP/1.0 400"), answer[:80])
                self.assertIn(b'"error"', answer)

    def test_a_body_over_the_cap_is_413_and_is_not_read(self) -> None:
        # Only the header is sent: the server answers without waiting for a body.
        head = b"POST /forecast HTTP/1.1\r\nHost: x\r\nConnection: close\r\n"
        length = str(service.MAX_BODY + 1).encode()
        answer = self._raw(head + b"Content-Length: " + length + b"\r\n\r\n")
        self.assertTrue(answer.startswith(b"HTTP/1.0 413"), answer[:80])

    def test_a_request_the_model_refuses_is_400(self) -> None:
        from tests.test_service import RefusingModel

        server = serve(RefusingModel(), "127.0.0.1", 0)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            request = urllib.request.Request(
                f"http://127.0.0.1:{server.server_address[1]}/forecast",
                data=b'{"context": [1.0, 2.0, 3.0], "horizon": 1}',
                method="POST",
            )
            with self.assertRaises(urllib.error.HTTPError) as raised:
                urllib.request.urlopen(request, timeout=5)
            with raised.exception as error:
                self.assertEqual(error.code, 400)
        finally:
            server.shutdown()
            server.server_close()

    def test_idle_connections_time_out(self) -> None:
        # A client that connects and says nothing must not hold a thread.
        self.assertEqual(make_handler(StubModel()).timeout, REQUEST_TIMEOUT_S)
        self.assertTrue(0 < REQUEST_TIMEOUT_S <= 60)


class BindTests(unittest.TestCase):
    def test_parse_bind(self) -> None:
        self.assertEqual(parse_bind("0.0.0.0:8700"), ("0.0.0.0", 8700))
        self.assertEqual(parse_bind("[::1]:9"), ("[::1]", 9))
        with self.assertRaises(ValueError):
            parse_bind("8700")


if __name__ == "__main__":
    unittest.main()

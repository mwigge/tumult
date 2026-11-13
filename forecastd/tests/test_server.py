"""The stdlib server end to end over a loopback socket."""

from __future__ import annotations

import json
import threading
import unittest
import urllib.error
import urllib.request

from forecastd.__main__ import parse_bind
from forecastd.model import StubModel
from forecastd.server import serve


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


class BindTests(unittest.TestCase):
    def test_parse_bind(self) -> None:
        self.assertEqual(parse_bind("0.0.0.0:8700"), ("0.0.0.0", 8700))
        self.assertEqual(parse_bind("[::1]:9"), ("[::1]", 9))
        with self.assertRaises(ValueError):
            parse_bind("8700")


if __name__ == "__main__":
    unittest.main()

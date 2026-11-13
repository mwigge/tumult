"""The FastAPI application, driven through the ASGI interface without a
socket. Skipped only when FastAPI is not installed (the stdlib server in
``test_server`` covers the same contract)."""

from __future__ import annotations

import asyncio
import importlib.util
import json
import unittest

from forecastd.model import StubModel

HAVE_FASTAPI = importlib.util.find_spec("fastapi") is not None


def call(app, method: str, path: str, body: bytes = b"") -> tuple[int, dict]:
    """One request against an ASGI app; returns the status and JSON body."""
    scope = {
        "type": "http",
        "asgi": {"version": "3.0"},
        "http_version": "1.1",
        "method": method,
        "scheme": "http",
        "path": path,
        "raw_path": path.encode(),
        "query_string": b"",
        "root_path": "",
        "headers": [(b"content-type", b"application/json"), (b"content-length", str(len(body)).encode())],
        "client": ("127.0.0.1", 1),
        "server": ("127.0.0.1", 8700),
    }
    sent: list[dict] = []
    pending = [{"type": "http.request", "body": body, "more_body": False}]

    async def receive() -> dict:
        if pending:
            return pending.pop(0)
        return {"type": "http.disconnect"}

    async def send(message: dict) -> None:
        sent.append(message)

    asyncio.run(app(scope, receive, send))
    status = next(m["status"] for m in sent if m["type"] == "http.response.start")
    payload = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return status, json.loads(payload)


@unittest.skipUnless(HAVE_FASTAPI, "fastapi is not installed")
class AsgiTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        from forecastd.asgi import create_app

        cls.app = create_app(StubModel())

    def test_healthz(self) -> None:
        status, body = call(self.app, "GET", "/healthz")
        self.assertEqual(status, 200)
        self.assertEqual(body["status"], "ok")
        self.assertEqual(body["model"], "stub")

    def test_forecast_reads_the_json_body(self) -> None:
        request = {"context": [1.0, 2.0, 3.0, 4.0], "horizon": 3, "quantile_levels": [0.1, 0.5, 0.9]}
        status, body = call(self.app, "POST", "/forecast", json.dumps(request).encode())
        self.assertEqual(status, 200, body)
        self.assertEqual(body["levels"], [0.1, 0.5, 0.9])
        self.assertEqual(len(body["quantiles"][0]), 3)
        self.assertEqual(body["model"]["model"], "stub")

    def test_a_bad_request_is_400(self) -> None:
        status, body = call(self.app, "POST", "/forecast", json.dumps({"context": [], "horizon": 1}).encode())
        self.assertEqual(status, 400, body)
        self.assertIn("detail", body)

    def test_a_body_that_is_not_json_is_400(self) -> None:
        status, body = call(self.app, "POST", "/forecast", b"not json")
        self.assertEqual(status, 400, body)


if __name__ == "__main__":
    unittest.main()

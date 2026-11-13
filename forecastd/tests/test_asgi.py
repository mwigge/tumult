"""The FastAPI application, driven through the ASGI interface without a
socket. Skipped only when FastAPI is not installed (the stdlib server in
``test_server`` covers the same contract)."""

from __future__ import annotations

import asyncio
import importlib.util
import json
import threading
import unittest
from unittest import mock

from forecastd import service
from forecastd.model import StubModel

HAVE_FASTAPI = importlib.util.find_spec("fastapi") is not None


def call(
    app,
    method: str,
    path: str,
    body: bytes = b"",
    content_length: bytes | None = b"",
    chunks: list[bytes] | None = None,
) -> tuple[int, dict]:
    """One request against an ASGI app; returns the status and JSON body.
    ``content_length`` overrides the header (``None`` leaves it out) and
    ``chunks`` sends the body in several messages."""
    headers = [(b"content-type", b"application/json")]
    if content_length == b"":
        content_length = str(len(body)).encode()
    if content_length is not None:
        headers.append((b"content-length", content_length))
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
        "headers": headers,
        "client": ("127.0.0.1", 1),
        "server": ("127.0.0.1", 8700),
    }
    sent: list[dict] = []
    parts = chunks if chunks is not None else [body]
    pending = [
        {"type": "http.request", "body": part, "more_body": i + 1 < len(parts)}
        for i, part in enumerate(parts)
    ]

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

    def test_an_announced_body_over_the_cap_is_413(self) -> None:
        announced = str(service.MAX_BODY + 1).encode()
        status, body = call(self.app, "POST", "/forecast", b"{}", content_length=announced)
        self.assertEqual(status, 413, body)

    def test_a_streamed_body_over_the_cap_is_413(self) -> None:
        # No content-length to trust: the cap holds while reading.
        request = json.dumps({"context": [1.0] * 64, "horizon": 1}).encode()
        with mock.patch.object(service, "MAX_BODY", 100):
            status, body = call(
                self.app,
                "POST",
                "/forecast",
                content_length=None,
                chunks=[request[:90], request[90:]],
            )
        self.assertEqual(status, 413, body)
        status, body = call(
            self.app, "POST", "/forecast", content_length=None, chunks=[request[:90], request[90:]]
        )
        self.assertEqual(status, 200, body)

    def test_a_content_length_that_is_not_a_number_is_400(self) -> None:
        status, body = call(self.app, "POST", "/forecast", b"{}", content_length=b"lots")
        self.assertEqual(status, 400, body)

    def test_inference_runs_off_the_event_loop(self) -> None:
        seen: list[int] = []

        class Recording(StubModel):
            def forecast(self, context, horizon, past_covariates, known_covariates):
                seen.append(threading.get_ident())
                return super().forecast(context, horizon, past_covariates, known_covariates)

        from forecastd.asgi import create_app

        app = create_app(Recording())
        request = json.dumps({"context": [1.0, 2.0, 3.0], "horizon": 1}).encode()
        status, body = call(app, "POST", "/forecast", request)
        self.assertEqual(status, 200, body)
        # ``call`` runs the event loop on this thread.
        self.assertEqual(len(seen), 1)
        self.assertNotEqual(seen[0], threading.get_ident())

    def test_a_request_the_model_refuses_is_400(self) -> None:
        from forecastd.asgi import create_app
        from tests.test_service import RefusingModel

        request = json.dumps({"context": [1.0, 2.0, 3.0], "horizon": 1}).encode()
        status, body = call(create_app(RefusingModel()), "POST", "/forecast", request)
        self.assertEqual(status, 400, body)


if __name__ == "__main__":
    unittest.main()

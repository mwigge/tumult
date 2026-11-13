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


def call(app, method: str, path: str, body: bytes = b"", **how) -> tuple[int, dict]:
    """One request against an ASGI app on an event loop of its own; see
    ``acall``."""
    status, payload, _ = asyncio.run(acall(app, method, path, body, **how))
    return status, payload


async def acall(
    app,
    method: str,
    path: str,
    body: bytes = b"",
    content_length: bytes | None = b"",
    chunks: list[bytes] | None = None,
    stall: bool = False,
) -> tuple[int, dict, dict]:
    """One request against an ASGI app; returns the status, the JSON body
    and the response headers. ``content_length`` overrides the header
    (``None`` leaves it out), ``chunks`` sends the body in several messages
    and ``stall`` never sends the last of them."""
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
        if stall and len(pending) == 1:
            await asyncio.Event().wait()
        if pending:
            return pending.pop(0)
        return {"type": "http.disconnect"}

    async def send(message: dict) -> None:
        sent.append(message)

    await app(scope, receive, send)
    start = next(m for m in sent if m["type"] == "http.response.start")
    headers = {name.decode().lower(): value.decode() for name, value in start["headers"]}
    payload = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], json.loads(payload), headers


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
        self.assertEqual(body["detail"], "the model refused the request")

    def test_a_full_house_answers_503_and_takes_requests_again(self) -> None:
        from forecastd.asgi import create_app
        from forecastd.service import Gate
        from tests.test_server import GOOD, HeldModel

        model = HeldModel()
        self.addCleanup(model.go.set)
        app = create_app(model, Gate(2))

        async def scenario() -> tuple[list, tuple, tuple]:
            loop = asyncio.get_running_loop()
            held = [asyncio.ensure_future(acall(app, "POST", "/forecast", GOOD)) for _ in range(2)]
            for _ in held:
                reached = await loop.run_in_executor(None, model.entered.acquire, True, 10)
                self.assertTrue(reached, "both requests reach the model")
            turned_away = await acall(app, "POST", "/forecast", GOOD)
            health = await acall(app, "GET", "/healthz")
            model.go.set()
            return await asyncio.gather(*held), turned_away, health

        with self.assertLogs("forecastd", level="WARNING") as seen:
            done, turned_away, health = asyncio.run(scenario())
        self.assertEqual([status for status, _, _ in done], [200, 200])
        status, body, headers = turned_away
        self.assertEqual(status, 503, body)
        self.assertIn("busy", body["detail"])
        self.assertEqual(headers.get("retry-after"), str(service.RETRY_AFTER_S))
        self.assertEqual(health[0], 200)
        self.assertEqual([record.event for record in seen.records], ["forecast.busy"])
        self.assertFalse(model.entered.acquire(blocking=False), "the third never reached the model")
        for _ in range(3):
            self.assertEqual(call(app, "POST", "/forecast", GOOD)[0], 200)

    def test_every_kind_of_answer_gives_its_slot_back(self) -> None:
        from forecastd.asgi import create_app
        from forecastd.service import Gate

        class Moody(StubModel):
            def forecast(self, context, horizon, past_covariates, known_covariates):
                if horizon == 2:
                    raise ValueError("not this horizon")
                if horizon == 3:
                    raise RuntimeError("broken")
                return super().forecast(context, horizon, past_covariates, known_covariates)

        app = create_app(Moody(), Gate(1))
        announced = str(service.MAX_BODY + 1).encode()
        with self.assertLogs("forecastd", level="WARNING"):
            for how, expected in (
                ({"body": b"not json"}, 400),
                ({"body": b"{}", "content_length": b"lots"}, 400),
                ({"body": b"{}", "content_length": announced}, 413),
                ({"body": b'{"context": [1.0, 2.0, 3.0], "horizon": 2}'}, 400),
                ({"body": b'{"context": [1.0, 2.0, 3.0], "horizon": 3}'}, 500),
                ({"body": b'{"context": [1.0, 2.0, 3.0], "horizon": 1}'}, 200),
            ):
                with self.subTest(how=how):
                    self.assertEqual(call(app, "POST", "/forecast", **how)[0], expected)

    def test_a_body_nested_too_deeply_is_400(self) -> None:
        for body in (b"[" * 200_000, b'{"context":' + b"[" * 200_000, b'{"a":' * 100_000):
            status, answer = call(self.app, "POST", "/forecast", body)
            self.assertEqual(status, 400, answer)

    def test_a_body_that_stalls_is_408_and_gives_its_slot_back(self) -> None:
        from forecastd import asgi
        from forecastd.service import Gate

        app = asgi.create_app(StubModel(), Gate(1))
        request = b'{"context": [1.0, 2.0, 3.0], "horizon": 1}'
        with mock.patch.object(asgi, "BODY_TIMEOUT_S", 0.05):
            status, body = call(
                app, "POST", "/forecast", chunks=[request[:10], request[10:]], stall=True
            )
        self.assertEqual(status, 408, body)
        self.assertEqual(call(app, "POST", "/forecast", request)[0], 200)

    def test_a_request_is_one_access_event(self) -> None:
        with self.assertLogs("forecastd", level="INFO") as seen:
            status, _ = call(self.app, "GET", "/healthz")
            refused, _ = call(self.app, "POST", "/forecast", b"not json")
        self.assertEqual((status, refused), (200, 400))
        self.assertEqual([record.event for record in seen.records], ["http.access"] * 2)
        first, second = (record.fields for record in seen.records)
        self.assertEqual(first, {"client": "127.0.0.1", "method": "GET", "path": "/healthz", "status": 200})
        self.assertEqual(second["status"], 400)
        self.assertEqual(second["path"], "/forecast")
        for record in seen.records:
            self.assertEqual(record.getMessage(), "request served")
            self.assertEqual(record.args, ())

    def test_a_model_that_crashes_is_500_and_one_event(self) -> None:
        from forecastd.asgi import create_app

        class Crashing(StubModel):
            def forecast(self, context, horizon, past_covariates, known_covariates):
                raise RuntimeError("tensor /weights/x\nwent wrong")

        request = json.dumps({"context": [1.0, 2.0, 3.0], "horizon": 1}).encode()
        with self.assertLogs("forecastd", level="ERROR") as seen:
            status, body = call(create_app(Crashing()), "POST", "/forecast", request)
        self.assertEqual(status, 500, body)
        self.assertEqual(body, {"detail": "forecast failed"})
        (record,) = seen.records
        self.assertEqual(record.event, "forecast.failed")
        self.assertEqual(record.getMessage(), "forecast failed")


if __name__ == "__main__":
    unittest.main()

"""The stdlib server end to end over a loopback socket."""

from __future__ import annotations

import json
import socket
import threading
import unittest
import urllib.error
import urllib.request

from forecastd import service
from forecastd.__main__ import parse_bind
from forecastd.model import StubModel
from forecastd.server import BODY_DEADLINE_S, REQUEST_TIMEOUT_S, make_handler, serve
from forecastd.service import Gate

GOOD = b'{"context": [1.0, 2.0, 3.0], "horizon": 1}'


class HeldModel(StubModel):
    """A model that holds every forecast until the test lets it go."""

    def __init__(self) -> None:
        super().__init__()
        self.entered = threading.Semaphore(0)
        self.go = threading.Event()

    def forecast(self, context, horizon, past_covariates, known_covariates):
        self.entered.release()
        if not self.go.wait(timeout=20):
            raise RuntimeError("the test never released the model")
        return super().forecast(context, horizon, past_covariates, known_covariates)


def lowered(headers) -> dict[str, str]:
    return {name.lower(): value for name, value in headers.items()}


def post(base: str, body: bytes, path: str = "/forecast") -> tuple[int, dict, dict]:
    """POST and return the status, the JSON body and the headers."""
    request = urllib.request.Request(base + path, data=body, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return response.status, json.loads(response.read()), lowered(response.headers)
    except urllib.error.HTTPError as error:
        with error:
            return error.code, json.loads(error.read()), lowered(error.headers)


class QuarterClock:
    """A clock that moves a quarter of the body deadline every time it is
    read, and lets the test wait for a reading."""

    def __init__(self) -> None:
        self.readings = 0
        self.read = threading.Semaphore(0)

    def __call__(self) -> float:
        now = self.readings * BODY_DEADLINE_S / 4
        self.readings += 1
        self.read.release()
        return now


def drip(case: unittest.TestCase, base: str, clock: QuarterClock) -> bytes:
    """Announce a long body and send it a byte at a time, each byte only
    when the server has come back to the clock for the one before. Returns
    what the server answered before it closed the connection."""
    port = int(base.rsplit(":", 1)[1])
    with socket.create_connection(("127.0.0.1", port), timeout=20) as client:
        client.sendall(b"POST /forecast HTTP/1.1\r\nHost: x\r\nContent-Length: 1000\r\n\r\n{")
        # The deadline is set, then the clock is read before every piece.
        for readings in (3, 1):
            for _ in range(readings):
                case.assertTrue(clock.read.acquire(timeout=10), "the server reads its clock")
            client.sendall(b" ")
        answer = bytearray()
        while piece := client.recv(4096):
            answer.extend(piece)
        return bytes(answer)


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


class GateTests(unittest.TestCase):
    """At most ``limit`` forecast requests in hand; the next one is told
    the service is busy instead of queueing without bound."""

    def start(self, model: StubModel, limit: int, clock: QuarterClock | None = None) -> str:
        server = serve(model, "127.0.0.1", 0, Gate(limit), *([clock] if clock else []))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return f"http://127.0.0.1:{server.server_address[1]}"

    def test_a_full_house_answers_503_and_takes_requests_again(self) -> None:
        model = HeldModel()
        self.addCleanup(model.go.set)
        base = self.start(model, 2)
        answers: list[int] = []
        held = [
            threading.Thread(target=lambda: answers.append(post(base, GOOD)[0])) for _ in range(2)
        ]
        for thread in held:
            thread.start()
        for _ in held:
            self.assertTrue(model.entered.acquire(timeout=10), "both requests reach the model")

        with self.assertLogs("forecastd", level="WARNING") as seen:
            status, body, headers = post(base, GOOD)
        self.assertEqual(status, 503, body)
        self.assertIn("busy", body["error"])
        self.assertEqual(headers.get("retry-after"), str(service.RETRY_AFTER_S))
        self.assertEqual([record.event for record in seen.records], ["forecast.busy"])
        self.assertEqual(seen.records[0].fields, {"limit": 2})
        # The model saw two requests, not three, and health is not gated.
        self.assertFalse(model.entered.acquire(blocking=False))
        with urllib.request.urlopen(base + "/healthz", timeout=5) as response:
            self.assertEqual(response.status, 200)

        model.go.set()
        for thread in held:
            thread.join(timeout=20)
        self.assertEqual(answers, [200, 200])
        for _ in range(3):
            self.assertEqual(post(base, GOOD)[0], 200)

    def test_a_busy_answer_arrives_whole_whatever_the_body(self) -> None:
        model = HeldModel()
        self.addCleanup(model.go.set)
        base = self.start(model, 1)
        holder = threading.Thread(target=post, args=(base, GOOD))
        holder.start()
        self.assertTrue(model.entered.acquire(timeout=10))
        # A megabyte the server has no slot for: read and dropped, so the
        # answer is not lost to a reset connection.
        large = json.dumps({"context": [1.5] * 200_000, "horizon": 1}).encode()
        for _ in range(3):
            status, body, _ = post(base, large)
            self.assertEqual(status, 503, body)
        model.go.set()
        holder.join(timeout=20)

    def test_a_dripped_body_is_408_at_the_deadline_and_gives_its_slot_back(self) -> None:
        # Three bytes of a thousand, each well inside the idle timeout: the
        # whole body has a deadline of its own.
        clock = QuarterClock()
        base = self.start(StubModel(), 1, clock)
        with self.assertLogs("forecastd", level="WARNING") as seen:
            answer = drip(self, base, clock)
        head, _, body = answer.partition(b"\r\n\r\n")
        self.assertIn(b" 408 ", head.split(b"\r\n", 1)[0])
        self.assertIn("did not arrive in time", json.loads(body)["error"])
        self.assertEqual([record.event for record in seen.records], ["http.body_timeout"])
        self.assertEqual(seen.records[0].fields["deadline_s"], BODY_DEADLINE_S)
        self.assertEqual(seen.records[0].fields["received"], 3)
        self.assertEqual(post(base, GOOD)[0], 200)

    def test_a_dripped_body_with_no_slot_is_503_at_the_deadline(self) -> None:
        model = HeldModel()
        self.addCleanup(model.go.set)
        clock = QuarterClock()
        base = self.start(model, 1, clock)
        holder = threading.Thread(target=post, args=(base, GOOD))
        holder.start()
        self.assertTrue(model.entered.acquire(timeout=10))
        # The held request has read the clock for its own body.
        while clock.read.acquire(blocking=False):
            pass
        with self.assertLogs("forecastd", level="WARNING"):
            answer = drip(self, base, clock)
        self.assertIn(b" 503 ", answer.split(b"\r\n", 1)[0])
        model.go.set()
        holder.join(timeout=20)

    def test_every_kind_of_answer_gives_its_slot_back(self) -> None:
        class Moody(StubModel):
            def forecast(self, context, horizon, past_covariates, known_covariates):
                if horizon == 2:
                    raise ValueError("not this horizon")
                if horizon == 3:
                    raise RuntimeError("broken")
                return super().forecast(context, horizon, past_covariates, known_covariates)

        base = self.start(Moody(), 1)
        with self.assertLogs("forecastd", level="WARNING"):
            for body, expected in (
                (b"not json", 400),
                (b'{"context": [], "horizon": 1}', 400),
                (b'{"context": [1.0, 2.0, 3.0], "horizon": 2}', 400),
                (b'{"context": [1.0, 2.0, 3.0], "horizon": 3}', 500),
                (GOOD, 200),
            ):
                with self.subTest(body=body):
                    self.assertEqual(post(base, body)[0], expected)
        self.assertEqual(post(base, b"{}", "/nope")[0], 404)
        self.assertEqual(post(base, GOOD)[0], 200)

    def test_a_body_nested_too_deeply_is_400(self) -> None:
        base = self.start(StubModel(), 1)
        for body in (b"[" * 200_000, b'{"context":' + b"[" * 200_000, b'{"a":' * 100_000):
            status, answer, _ = post(base, body)
            self.assertEqual(status, 400, answer)
            self.assertIn("error", answer)
        self.assertEqual(post(base, GOOD)[0], 200)

    def test_the_default_gate_is_the_documented_limit(self) -> None:
        self.assertEqual(service.DEFAULT_MAX_CONCURRENT, 4)
        server = serve(StubModel(), "127.0.0.1", 0)
        self.addCleanup(server.server_close)
        self.assertEqual(server.gate.limit, 4)


class BindTests(unittest.TestCase):
    def test_parse_bind(self) -> None:
        self.assertEqual(parse_bind("0.0.0.0:8700"), ("0.0.0.0", 8700))
        self.assertEqual(parse_bind("[::1]:9"), ("[::1]", 9))
        with self.assertRaises(ValueError):
            parse_bind("8700")


if __name__ == "__main__":
    unittest.main()

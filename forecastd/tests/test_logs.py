"""The log format: one JSON object per line, values in fields, nothing a
client or a model wrote able to forge a line or flood one."""

from __future__ import annotations

import io
import json
import logging
import os
import re
import subprocess
import sys
import threading
import unittest
import urllib.error
import urllib.request
import warnings
from pathlib import Path
from unittest import mock

from forecastd import logs
from forecastd.logs import JsonFormatter, TextFormatter, clip, event, log_format, log_level
from forecastd.model import StubModel
from forecastd.server import serve
from forecastd.service import BadRequest, handle_forecast

PACKAGE = Path(logs.__file__).resolve().parent
REQUIRED = {"timestamp", "level", "logger", "event", "message", "fields"}
TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z")


def capture(formatter: logging.Formatter) -> tuple[logging.Logger, io.StringIO]:
    """A logger of its own writing through ``formatter`` into a buffer."""
    stream = io.StringIO()
    handler = logging.StreamHandler(stream)
    handler.setFormatter(formatter)
    logger = logging.Logger("forecastd.test", logging.DEBUG)
    logger.addHandler(handler)
    return logger, stream


def lines(stream: io.StringIO) -> list[str]:
    return stream.getvalue().splitlines()


class JsonFormatterTests(unittest.TestCase):
    def test_a_record_is_one_object_with_the_required_keys(self) -> None:
        logger, stream = capture(JsonFormatter())
        event(logger, logging.INFO, "model.loaded", "model loaded", size="5M", threads=4)
        (line,) = lines(stream)
        entry = json.loads(line)
        self.assertEqual(set(entry), REQUIRED)
        self.assertEqual(entry["level"], "INFO")
        self.assertEqual(entry["logger"], "forecastd.test")
        self.assertEqual(entry["event"], "model.loaded")
        self.assertEqual(entry["message"], "model loaded")
        self.assertEqual(entry["fields"], {"size": "5M", "threads": 4})

    def test_the_timestamp_is_rfc_3339_in_utc(self) -> None:
        logger, stream = capture(JsonFormatter())
        event(logger, logging.INFO, "tick", "tick")
        self.assertRegex(json.loads(lines(stream)[0])["timestamp"], TIMESTAMP)
        record = logging.LogRecord("forecastd", logging.INFO, "", 0, "tick", (), None)
        record.created = 1_700_000_000.1234
        entry = json.loads(JsonFormatter().format(record))
        self.assertEqual(entry["timestamp"], "2023-11-14T22:13:20.123Z")

    def test_arguments_of_a_plain_record_are_fields_not_message(self) -> None:
        # What uvicorn and the warnings module hand over.
        logger, stream = capture(JsonFormatter())
        logger.warning("started %s on port %d", "server", 8700)
        entry = json.loads(lines(stream)[0])
        self.assertEqual(entry["message"], "started %s on port %d")
        self.assertEqual(entry["event"], "log")
        self.assertEqual(entry["fields"], {"args": ["server", 8700]})

    def test_a_newline_cannot_forge_a_second_line(self) -> None:
        forged = 'x\n{"timestamp":"now","level":"INFO","event":"model.loaded"}\r\n\x1b[2J\x00'
        logger, stream = capture(JsonFormatter())
        event(logger, logging.WARNING, "model.refused", "the model refused a request", detail=forged)
        logger.warning(forged)
        logger.warning("%s", forged)
        out = lines(stream)
        self.assertEqual(len(out), 3, out)
        for line in out:
            self.assertNotRegex(line, r"[\x00-\x1f\x7f]")
            self.assertEqual(set(json.loads(line)), REQUIRED)
        self.assertEqual(json.loads(out[0])["fields"]["detail"], forged)
        self.assertEqual(json.loads(out[1])["message"], forged)
        self.assertEqual(json.loads(out[2])["fields"]["args"], [forged])

    def test_a_line_separator_outside_ascii_is_escaped_too(self) -> None:
        logger, stream = capture(JsonFormatter())
        event(logger, logging.INFO, "e", "m", detail="a b\u0085c")
        raw = stream.getvalue()
        self.assertEqual(raw.count("\n"), 1)
        self.assertTrue(raw.isascii())
        self.assertEqual(json.loads(raw)["fields"]["detail"], "a b\u0085c")

    def test_long_strings_are_truncated(self) -> None:
        logger, stream = capture(JsonFormatter())
        huge = "A" * 100_000
        event(
            logger,
            logging.WARNING,
            "model.refused",
            "the model refused a request",
            detail=huge,
            nested={"inner": [huge], huge: 1},
            other=ValueError(huge),
        )
        (line,) = lines(stream)
        self.assertLess(len(line), 8 * logs.MAX_STRING)
        fields = json.loads(line)["fields"]
        self.assertTrue(fields["detail"].startswith("A" * logs.MAX_STRING))
        self.assertTrue(fields["detail"].endswith(logs.TRUNCATED))
        self.assertEqual(len(fields["detail"]), logs.MAX_STRING + len(logs.TRUNCATED))
        self.assertLessEqual(len(fields["nested"]["inner"][0]), logs.MAX_STRING + len(logs.TRUNCATED))
        self.assertTrue(all(len(key) < 200 for key in fields["nested"]))
        self.assertLessEqual(len(fields["other"]), logs.MAX_STRING + len(logs.TRUNCATED))
        self.assertEqual(clip("short"), "short")

    def test_a_long_collection_is_cut_short(self) -> None:
        logger, stream = capture(JsonFormatter())
        event(logger, logging.INFO, "e", "m", many=list(range(10_000)))
        self.assertEqual(len(json.loads(lines(stream)[0])["fields"]["many"]), logs.MAX_ITEMS)

    def test_values_json_cannot_hold_become_strings(self) -> None:
        logger, stream = capture(JsonFormatter())
        event(
            logger,
            logging.INFO,
            "e",
            "m",
            nan=float("nan"),
            big=10**5000,
            thing=object,
            raw=b"\xff\n",
            deep=[[[[[["bottom"]]]]]],
        )
        (line,) = lines(stream)
        fields = json.loads(line)["fields"]
        self.assertEqual(fields["nan"], "nan")
        self.assertIsInstance(fields["big"], str)
        self.assertIsInstance(fields["thing"], str)
        self.assertIsInstance(fields["raw"], str)

    def test_an_exception_becomes_error_fields(self) -> None:
        logger, stream = capture(JsonFormatter())
        try:
            raise ValueError("bad\nshape " + "B" * 50_000)
        except ValueError:
            event(logger, logging.ERROR, "forecast.failed", "forecast failed", exc_info=True)
        (line,) = lines(stream)
        entry = json.loads(line)
        fields = entry["fields"]
        self.assertEqual(entry["message"], "forecast failed")
        self.assertEqual(fields["error.type"], "ValueError")
        self.assertTrue(fields["error.message"].startswith("bad\nshape BBB"))
        self.assertLessEqual(len(fields["error.message"]), logs.MAX_STRING + len(logs.TRUNCATED))
        self.assertIn("Traceback", fields["error.stack"])
        self.assertLessEqual(len(fields["error.stack"]), logs.MAX_STACK + len(logs.TRUNCATED))

    def test_a_broken_str_does_not_lose_the_line(self) -> None:
        class Mute(Exception):
            def __str__(self) -> str:
                raise RuntimeError("no words")

        class Key:
            def __str__(self) -> str:
                raise RuntimeError("no words")

        logger, stream = capture(JsonFormatter())
        try:
            raise Mute
        except Mute as error:
            event(logger, logging.ERROR, "forecast.failed", "failed", exc_info=True, cause=error)
        event(logger, logging.INFO, "e", "m", table={Key(): 1})
        logger.info(Key())
        failed, keyed, plain = (json.loads(line) for line in lines(stream))
        mute = "<Mute: str failed with RuntimeError>"
        self.assertEqual(failed["event"], "forecast.failed")
        self.assertEqual(failed["fields"]["error.type"], "Mute")
        self.assertEqual(failed["fields"]["error.message"], mute)
        self.assertEqual(failed["fields"]["cause"], mute)
        self.assertEqual(list(keyed["fields"]["table"]), ["<Key: str failed with RuntimeError>"])
        self.assertEqual(plain["message"], "<Key: str failed with RuntimeError>")

    def test_a_record_that_cannot_be_formatted_leaves_a_fallback_line(self) -> None:
        class Broken(dict):
            def items(self):
                raise RuntimeError("no items")

        logger, stream = capture(JsonFormatter())
        event(logger, logging.WARNING, "forecast.busy", "turned away", detail=Broken())
        (line,) = lines(stream)
        entry = json.loads(line)
        self.assertEqual(set(entry), REQUIRED)
        self.assertRegex(entry["timestamp"], TIMESTAMP)
        self.assertEqual((entry["level"], entry["logger"]), ("WARNING", "forecastd.test"))
        self.assertEqual(entry["event"], logs.UNFORMATTABLE)
        self.assertEqual(entry["fields"], {"event": "forecast.busy", "error.type": "RuntimeError"})

        logger, stream = capture(TextFormatter())
        event(logger, logging.WARNING, "forecast.busy", "turned away", detail=Broken())
        (line,) = lines(stream)
        self.assertIn(f" WARNING forecastd.test {logs.UNFORMATTABLE} ", line)
        self.assertIn('event="forecast.busy"', line)

    def test_a_url_in_an_error_loses_its_query_string_and_credentials(self) -> None:
        # What a failed download says: the hub's signed URL, word for word.
        url = "https://cdn.example/repo/model.safetensors"
        logger, stream = capture(JsonFormatter())
        # Built here: the stack quotes the source of the lines that raise.
        signed = f"{url}?X-Amz-Signature=SECRET&Expires=1"
        with_token = f"'{url}?token=hf_SECRET' or https://me:hf_SECRET@hub.example/x"
        try:
            try:
                raise OSError(f"403 Client Error: Forbidden for url: {signed} (Request ID: 7)")
            except OSError as inner:
                raise RuntimeError(f"cannot fetch {with_token}") from inner
        except RuntimeError:
            event(logger, logging.ERROR, "startup.failed", "cannot start", exc_info=True)
        (line,) = lines(stream)
        self.assertNotIn("SECRET", line)
        self.assertNotIn("Expires", line)
        fields = json.loads(line)["fields"]
        self.assertEqual(
            fields["error.message"],
            f"cannot fetch '{url}?[redacted]' or https://[redacted]@hub.example/x",
        )
        self.assertIn(f"for url: {url}?[redacted] (Request ID: 7)", fields["error.stack"])

    def test_a_field_named_like_a_record_attribute_is_kept(self) -> None:
        logger, stream = capture(JsonFormatter())
        event(logger, logging.INFO, "e", "m", name="n", args="a", message="x", path="/forecast")
        fields = json.loads(lines(stream)[0])["fields"]
        self.assertEqual(fields, {"name": "n", "args": "a", "message": "x", "path": "/forecast"})


class TextFormatterTests(unittest.TestCase):
    def test_text_is_one_line_with_the_same_parts(self) -> None:
        logger, stream = capture(TextFormatter())
        event(logger, logging.WARNING, "model.refused", "the model refused a request", detail="a\nb")
        logger.warning("multi\nline %s", "x\ny")
        out = lines(stream)
        self.assertEqual(len(out), 2, out)
        self.assertRegex(out[0], r"^\d{4}-\d{2}-\d{2}T[\d:.]+Z WARNING forecastd\.test model\.refused ")
        self.assertIn('detail="a\\nb"', out[0])
        self.assertIn("the model refused a request", out[0])
        for line in out:
            self.assertNotRegex(line, r"[\x00-\x1f\x7f]")


class SettingsTests(unittest.TestCase):
    def test_json_is_the_default_format(self) -> None:
        self.assertEqual(log_format({}), "json")
        self.assertEqual(log_format({"FORECASTD_LOG_FORMAT": ""}), "json")
        self.assertEqual(log_format({"FORECASTD_LOG_FORMAT": "json"}), "json")
        self.assertEqual(log_format({"FORECASTD_LOG_FORMAT": " TEXT "}), "text")

    def test_an_unknown_format_or_level_is_refused(self) -> None:
        with self.assertRaises(ValueError) as raised:
            log_format({"FORECASTD_LOG_FORMAT": "xml"})
        self.assertIn("FORECASTD_LOG_FORMAT", str(raised.exception))
        self.assertEqual(log_level({}), logging.INFO)
        self.assertEqual(log_level({"FORECASTD_LOG_LEVEL": "debug"}), logging.DEBUG)
        with self.assertRaises(ValueError) as raised:
            log_level({"FORECASTD_LOG_LEVEL": "loud"})
        self.assertIn("FORECASTD_LOG_LEVEL", str(raised.exception))


class ConfigureTests(unittest.TestCase):
    """``configure`` owns the process-wide logging; everything it touches is
    put back afterwards."""

    def setUp(self) -> None:
        root = logging.getLogger()
        saved = (root.handlers[:], root.level, sys.excepthook, threading.excepthook)
        raising = logging.raiseExceptions
        uvicorn = {
            name: (logging.getLogger(name).handlers[:], logging.getLogger(name).propagate)
            for name in logs.UVICORN_LOGGERS
        }

        def restore() -> None:
            root.handlers[:], root.level, sys.excepthook, threading.excepthook = saved
            logging.captureWarnings(False)
            logging.raiseExceptions = raising
            for name, (handlers, propagate) in uvicorn.items():
                logging.getLogger(name).handlers[:] = handlers
                logging.getLogger(name).propagate = propagate

        self.addCleanup(restore)
        self.stream = io.StringIO()
        patched = mock.patch.object(sys, "stderr", self.stream)
        patched.start()
        self.addCleanup(patched.stop)

    def entries(self) -> list[dict]:
        return [json.loads(line) for line in lines(self.stream)]

    def test_everything_goes_through_the_one_formatter(self) -> None:
        access = logging.getLogger("uvicorn.access")
        access.addHandler(logging.StreamHandler(self.stream))
        access.propagate = False
        logs.configure({})
        event(logging.getLogger("forecastd"), logging.INFO, "model.loaded", "model loaded")
        logging.getLogger("uvicorn.error").warning("Started server process [%d]", 7)
        access.warning('%s - "%s %s HTTP/%s" %d', "127.0.0.1:1", "GET", "/healthz\nx", "1.1", 200)
        with warnings.catch_warnings():
            warnings.simplefilter("always")
            warnings.warn("careful\nnow", UserWarning, stacklevel=1)
        entries = self.entries()
        self.assertEqual(len(entries), 4, entries)
        self.assertEqual(
            [entry["logger"] for entry in entries],
            ["forecastd", "uvicorn.error", "uvicorn.access", "py.warnings"],
        )
        for entry in entries:
            self.assertEqual(set(entry), REQUIRED)

    def test_the_level_and_the_text_format_are_applied(self) -> None:
        logs.configure({"FORECASTD_LOG_FORMAT": "text", "FORECASTD_LOG_LEVEL": "warning"})
        log = logging.getLogger("forecastd")
        event(log, logging.INFO, "quiet", "not shown")
        event(log, logging.WARNING, "loud", "shown")
        (line,) = lines(self.stream)
        self.assertIn(" WARNING forecastd loud ", line)

    def test_an_uncaught_exception_is_a_log_line(self) -> None:
        logs.configure({})

        def boom() -> None:
            raise RuntimeError("thread\ndied")

        thread = threading.Thread(target=boom, name="worker")
        thread.start()
        thread.join()
        try:
            raise KeyError("main died")
        except KeyError:
            sys.excepthook(*sys.exc_info())
        entries = self.entries()
        self.assertEqual([entry["event"] for entry in entries], ["thread.crashed", "process.crashed"])
        self.assertEqual(entries[0]["fields"]["error.type"], "RuntimeError")
        self.assertEqual(entries[0]["fields"]["error.message"], "thread\ndied")
        self.assertEqual(entries[1]["fields"]["error.type"], "KeyError")

    def test_a_closed_stderr_does_not_fail_the_caller(self) -> None:
        # A line that cannot be written is lost; the request that logged it
        # is not, and ``logging`` does not try to complain on the same
        # closed stream.
        logs.configure({})
        self.stream.close()
        event(logging.getLogger("forecastd"), logging.INFO, "model.loaded", "model loaded")
        logging.getLogger("uvicorn.error").warning("Started server process [%d]", 7)
        self.assertFalse(logging.raiseExceptions)

    def test_a_bad_setting_changes_nothing(self) -> None:
        before = logging.getLogger().handlers[:]
        with self.assertRaises(ValueError):
            logs.configure({"FORECASTD_LOG_FORMAT": "xml"})
        self.assertEqual(logging.getLogger().handlers, before)


class Refusing(StubModel):
    def forecast(self, context, horizon, past_covariates, known_covariates):
        raise ValueError("/weights/secret/model.safetensors\nline two " + "C" * 50_000)


class ServiceLogTests(unittest.TestCase):
    def test_a_refusal_is_logged_in_fields_and_not_echoed(self) -> None:
        with self.assertLogs("forecastd", level="WARNING") as seen:
            with self.assertRaises(BadRequest) as raised:
                handle_forecast({"context": [1.0, 2.0, 3.0], "horizon": 2}, Refusing())
        # The client learns that the model said no, not what it said.
        self.assertEqual(str(raised.exception), "the model refused the request")
        (record,) = seen.records
        self.assertEqual(record.event, "model.refused")
        self.assertEqual(record.getMessage(), "the model refused a request")
        self.assertEqual(record.fields["error.type"], "ValueError")
        self.assertTrue(record.fields["error.message"].startswith("/weights/secret"))
        line = JsonFormatter().format(record)
        self.assertNotIn("\n", line)
        self.assertLess(len(line), 4 * logs.MAX_STRING)


class ServerLogTests(unittest.TestCase):
    def setUp(self) -> None:
        self.server = serve(StubModel(), "127.0.0.1", 0)
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def test_a_request_is_one_access_event(self) -> None:
        with self.assertLogs("forecastd", level="INFO") as seen:
            with urllib.request.urlopen(self.base + "/healthz?x=%0Ay", timeout=5) as response:
                self.assertEqual(response.status, 200)
            self.server.shutdown()
        (record,) = seen.records
        self.assertEqual(record.event, "http.access")
        self.assertEqual(record.getMessage(), "request served")
        self.assertEqual(record.args, ())
        self.assertEqual(record.fields["method"], "GET")
        self.assertEqual(record.fields["path"], "/healthz?x=%0Ay")
        self.assertEqual(record.fields["status"], 200)
        self.assertEqual(record.fields["client"], "127.0.0.1")

    def test_a_malformed_request_line_is_an_event_too(self) -> None:
        import socket

        with self.assertLogs("forecastd", level="INFO") as seen:
            with socket.create_connection(self.server.server_address, timeout=5) as sock:
                sock.sendall(b"NOT A REQUEST LINE AT ALL\r\n\r\n")
                while sock.recv(65536):
                    pass
            self.server.shutdown()
        self.assertTrue(seen.records)
        for record in seen.records:
            self.assertEqual(record.args, ())
            self.assertTrue(record.event.startswith("http."), record.event)
            self.assertNotIn("NOT A REQUEST", record.getMessage())

    def test_a_handler_that_crashes_is_logged_not_printed(self) -> None:
        stderr = io.StringIO()
        with mock.patch.object(sys, "stderr", stderr):
            with self.assertLogs("forecastd", level="ERROR") as seen:
                try:
                    raise RuntimeError("handler broke")
                except RuntimeError:
                    self.server.handle_error(None, ("127.0.0.1", 9))
        (record,) = seen.records
        self.assertEqual(record.event, "http.handler_failed")
        self.assertEqual(record.fields["client"], "127.0.0.1")
        self.assertEqual(stderr.getvalue(), "")


class ProcessTests(unittest.TestCase):
    """``python -m forecastd`` from the outside: stderr is JSON lines."""

    def run_main(self, **env: str) -> subprocess.CompletedProcess:
        base = {key: os.environ[key] for key in ("PATH", "HOME") if key in os.environ}
        return subprocess.run(
            [sys.executable, "-m", "forecastd"],
            cwd=PACKAGE.parent,
            env={**base, **env},
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )

    def test_a_start_that_fails_says_so_in_json(self) -> None:
        done = self.run_main(FORECASTD_MODEL="prophet\nforged")
        self.assertEqual(done.returncode, 1)
        self.assertEqual(done.stdout, "")
        entries = [json.loads(line) for line in done.stderr.splitlines()]
        self.assertEqual(len(entries), 1, done.stderr)
        self.assertEqual(set(entries[0]), REQUIRED)
        self.assertEqual(entries[0]["event"], "startup.failed")
        self.assertEqual(entries[0]["level"], "ERROR")
        self.assertEqual(entries[0]["fields"]["error.type"], "ValueError")

    def test_an_unknown_log_format_fails_the_start_in_json(self) -> None:
        done = self.run_main(FORECASTD_MODEL="stub", FORECASTD_LOG_FORMAT="xml")
        self.assertEqual(done.returncode, 1)
        (line,) = done.stderr.splitlines()
        entry = json.loads(line)
        self.assertEqual(entry["event"], "startup.failed")
        self.assertIn("FORECASTD_LOG_FORMAT", entry["fields"]["error.message"])

    def test_the_text_format_is_opt_in(self) -> None:
        done = self.run_main(FORECASTD_MODEL="prophet", FORECASTD_LOG_FORMAT="text")
        self.assertEqual(done.returncode, 1)
        (line,) = done.stderr.splitlines()
        self.assertIn(" ERROR forecastd startup.failed ", line)
        with self.assertRaises(ValueError):
            json.loads(line)


class SourceTests(unittest.TestCase):
    def test_the_package_logs_only_through_events(self) -> None:
        direct = re.compile(
            r"\b(?:log|logger|logging)\.(?:debug|info|warning|error|exception|critical|log)\("
            r"|(?<![\w.])print\("
            r"|sys\.std(?:err|out)\.write\("
        )
        for source in sorted(PACKAGE.glob("*.py")):
            if source.name == "logs.py":
                continue
            for number, line in enumerate(source.read_text().splitlines(), 1):
                with self.subTest(source=source.name, line=number):
                    self.assertIsNone(direct.search(line), line)

    def test_no_event_interpolates_its_message(self) -> None:
        call = re.compile(r"\bevent\(\s*log,\s*logging\.\w+,\s*(?P<name>[^,]+),\s*(?P<message>[^,)]+)")
        seen = 0
        for source in sorted(PACKAGE.glob("*.py")):
            for match in call.finditer(source.read_text()):
                seen += 1
                for part in (match["name"], match["message"]):
                    with self.subTest(source=source.name, part=part):
                        self.assertRegex(part.strip(), r'^"[^"{}%]*"$')
        self.assertGreaterEqual(seen, 6)


if __name__ == "__main__":
    unittest.main()

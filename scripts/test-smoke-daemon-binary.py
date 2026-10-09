#!/usr/bin/env python3
"""Hermetic process/HTTP regression checks for native daemon release smoke."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("smoke-daemon-binary.py")
FAKE = r"""
import json, os, signal, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
record = Path(RECORD)
with record.open("a") as out:
    out.write(json.dumps(dict(os.environ)) + "\n")
if FAIL:
    print("SENSITIVE-DIAGNOSTIC " + os.environ["KRONIKA_BOOTSTRAP_TOKEN"], file=sys.stderr)
    sys.exit(23)
db = Path(os.environ["TUMULT_LAKE_PATH"])
if not db.exists():
    db.write_text(os.environ["KRONIKA_BOOTSTRAP_TOKEN"])
token = db.read_text()
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        if not OPEN_AUTH and self.headers.get("Authorization") != "Bearer " + token:
            self.send_response(401); self.end_headers(); return
        self.send_response(200); self.end_headers()
        body = {"auth_required": True, "authenticated": True, "username": "admin"} if self.path == "/api/me" else {"metrics": [{"name": "hypothesis_pass_rate"}]}
        self.wfile.write(json.dumps(body).encode())
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
HTTPServer(("127.0.0.1", int(os.environ["KRONIKA_OTLP_HTTP_ADDR"].split(":")[-1])), Handler).serve_forever()
"""


class BinarySmokeTests(unittest.TestCase):
    def run_smoke(
        self, fail: bool = False, open_auth: bool = False
    ) -> tuple[subprocess.CompletedProcess[str], list[dict[str, str]]]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = root / "records"
            binary = root / "fake-daemon"
            binary.write_text(
                f"#!{sys.executable}\n"
                + FAKE.replace("RECORD", repr(str(record)))
                .replace("FAIL", repr(fail))
                .replace("OPEN_AUTH", repr(open_auth))
            )
            binary.chmod(0o700)
            metrics = root / "metrics"
            metrics.mkdir()
            env = dict(
                os.environ,
                HOME="/unexpected-home",
                DUCKDB_EXTENSION_DIRECTORY="/cached-extensions",
                KRONIKA_BOOTSTRAP_TOKEN="must-not-inherit",
                HTTPS_PROXY="http://invalid-proxy",
            )
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    str(binary),
                    "--metrics-dir",
                    str(metrics),
                    "--timeout",
                    "3",
                ],
                capture_output=True,
                text=True,
                env=env,
                timeout=15,
            )
            records = (
                [json.loads(line) for line in record.read_text().splitlines()]
                if record.exists()
                else []
            )
            return result, records

    def test_restart_uses_persisted_credentials_and_isolated_environment(self) -> None:
        result, records = self.run_smoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(records), 2)
        first, second = records
        self.assertEqual(first["TUMULT_LAKE_PATH"], second["TUMULT_LAKE_PATH"])
        self.assertNotIn("KRONIKA_BOOTSTRAP_TOKEN", second)
        self.assertNotIn("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD", second)
        for env in records:
            for name in ["HOME", "DUCKDB_EXTENSION_DIRECTORY", "HTTPS_PROXY"]:
                self.assertNotIn(name, env)
            self.assertTrue(env["KRONIKA_OTLP_HTTP_ADDR"].startswith("127.0.0.1:"))
        self.assertNotEqual(first["KRONIKA_BOOTSTRAP_TOKEN"], "must-not-inherit")
        self.assertNotIn(
            first["KRONIKA_BOOTSTRAP_TOKEN"], result.stdout + result.stderr
        )
        self.assertFalse(Path(first["TUMULT_LAKE_PATH"]).parent.exists())

    def test_unprotected_health_endpoint_fails_and_cleans_up(self) -> None:
        result, records = self.run_smoke(open_auth=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unauthenticated health request was accepted", result.stderr)
        self.assertEqual(len(records), 1)
        self.assertFalse(Path(records[0]["TUMULT_LAKE_PATH"]).parent.exists())

    def test_startup_failure_is_nonzero_without_leaking_child_logs(self) -> None:
        result, records = self.run_smoke(fail=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("23", result.stderr)
        self.assertNotIn("SENSITIVE-DIAGNOSTIC", result.stderr)
        self.assertEqual(len(records), 1)
        self.assertNotIn(records[0]["KRONIKA_BOOTSTRAP_TOKEN"], result.stderr)
        self.assertFalse(Path(records[0]["TUMULT_LAKE_PATH"]).parent.exists())


if __name__ == "__main__":
    unittest.main()

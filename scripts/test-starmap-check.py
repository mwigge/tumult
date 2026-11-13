#!/usr/bin/env python3
"""Hermetic checks of scripts/starmap-check.py against a fake daemon, and of
the Star Map part of scripts/demo-check.sh against a fake `docker`."""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

SCRIPT = Path(__file__).with_name("starmap-check.py")
DEMO_CHECK = Path(__file__).with_name("demo-check.sh")
PROJECT = "tumult-demo-check-kronika"
# Records each call (arguments and the Kronika variables) and plays the
# compose subcommands demo-check.sh uses.
FAKE_DOCKER = r"""
import json, os, sys, time
args = sys.argv[1:]
env = {k: v for k, v in os.environ.items() if k.startswith("KRONIKA_")}
with open(os.environ["FAKE_DOCKER_RECORD"], "a") as out:
    out.write(json.dumps({"args": args, "env": env}) + "\n")
if "up" in args:
    time.sleep(float(os.environ.get("FAKE_UP_SECONDS", "0")))
if "run" in args:
    print("=== seed: suite done — 6 passed, 2 deviated/failed")
"""


def node(node_id, kind, **over):
    base = {
        "id": node_id,
        "kind": kind,
        "gap": False,
        "evidence": {"failed": 0},
        "risks": {"open": 0, "suppressed": 0},
    }
    return {**base, **over}


def demo_map(suppressed):
    """The smallest map every structural line holds for."""
    nodes = [node("org:", "org", org_kind="org")]
    nodes += [node(f"org:team-{i}", "org", org_kind="team") for i in range(3)]
    nodes += [node(f"wl:demo/w{i}", "workload") for i in range(4)]
    nodes += [node(f"def:name/d{i}", "def") for i in range(8)]
    nodes.append(node("sys:demo/cache", "system", gap=True))
    nodes.append(
        node(
            "sys:demo/queue",
            "system",
            evidence={"failed": 1},
            risks={"open": 1 - suppressed, "suppressed": suppressed},
        )
    )
    edges = [
        {"from": "sys:demo/queue", "rel": "runs_on", "to": f"wl:demo/w{i}"}
        for i in range(4)
    ]
    edges += [
        {"from": "sys:demo/queue", "rel": "tested_by", "to": f"def:name/d{i}"}
        for i in range(8)
    ]
    counts = {"risks_open": 1 - suppressed, "risks_suppressed": suppressed}
    return {"nodes": nodes, "edges": edges, "counts": counts}


class FakeDaemon:
    """Serves a star map and a risk list; a suppress request moves the risk."""

    def __init__(self, suppressed=0, star=None):
        self.suppressed = suppressed
        self.star = star
        self.writes = []
        daemon = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def answer(self, body):
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps(body).encode())

            def do_GET(self):
                if self.path.startswith("/api/starmap"):
                    self.answer(daemon.star or demo_map(daemon.suppressed))
                    return
                suppression = {"reason": "accepted"} if daemon.suppressed else None
                self.answer({"risks": [{"id": "r_1", "suppression": suppression}]})

            def do_POST(self):
                length = int(self.headers.get("Content-Length", "0"))
                daemon.writes.append((self.path, json.loads(self.rfile.read(length))))
                daemon.suppressed = 1
                self.answer({"ok": True})

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


def check(daemon, *flags):
    return subprocess.run(
        [sys.executable, str(SCRIPT), daemon.url, "--wait", "0", *flags],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )


class StarMapCheckTests(unittest.TestCase):
    def test_without_write_nothing_is_suppressed_and_the_output_says_so(self):
        with FakeDaemon() as daemon:
            result = check(daemon)
            self.assertEqual(daemon.writes, [])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(
            sum(line.startswith("ok  ") for line in lines), 7, result.stdout
        )
        skipped = [line for line in lines if line.startswith("skip")]
        self.assertEqual(len(skipped), 1, result.stdout)
        self.assertIn("--write", skipped[0])
        self.assertNotIn("FAIL", result.stdout)

    def test_with_write_one_open_risk_is_suppressed(self):
        with FakeDaemon() as daemon:
            result = check(daemon, "--write")
            self.assertEqual(
                [path for path, _ in daemon.writes], ["/api/risks/r_1/suppress"]
            )
            self.assertEqual(daemon.writes[0][1]["expires_in_days"], 7)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("ok   star map: 1 suppressed risks on 1 nodes", result.stdout)
        self.assertNotIn("skip", result.stdout)

    def test_a_risk_suppressed_already_is_checked_without_a_write(self):
        for flags in ((), ("--write",)):
            with FakeDaemon(suppressed=1) as daemon:
                result = check(daemon, *flags)
                self.assertEqual(daemon.writes, [])
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("ok   star map: 1 suppressed risks on 1 nodes", result.stdout)

    def test_an_unexpected_answer_is_one_clear_failure(self):
        shapes = [
            ["not", "a", "map"],
            {"nodes": ["a string"], "edges": [], "counts": {}},
            {"nodes": None, "edges": [], "counts": {}},
            {"nodes": [], "edges": [[1, 2]], "counts": {}},
            {
                "nodes": [
                    {"id": "org:", "kind": "org", "gap": False, "evidence": None}
                ],
                "edges": [],
            },
        ]
        for star in shapes:
            with FakeDaemon(star=star) as daemon:
                result = check(daemon, "--write")
                self.assertEqual(daemon.writes, [])
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertEqual(result.stderr, "", "no traceback")
            lines = result.stdout.splitlines()
            self.assertEqual(len(lines), 1, result.stdout)
            self.assertTrue(lines[0].startswith("FAIL star map: "), lines[0])
            self.assertIn("did not answer as expected", lines[0])

    def test_a_daemon_that_is_not_there_is_one_clear_failure(self):
        with FakeDaemon() as daemon:
            pass
        result = check(daemon)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(result.stdout.splitlines()), 1, result.stdout)


class Checkout:
    """A copy of the scripts under a path with a space, and a fake `docker`."""

    def __init__(self, directory):
        self.root = Path(directory) / "a checkout"
        (self.root / "scripts").mkdir(parents=True)
        (self.root / "docker").mkdir()
        (self.root / "docker" / "docker-compose.kronika.yml").write_text(
            "services: {}\n"
        )
        for script in (DEMO_CHECK, SCRIPT):
            shutil.copy(script, self.root / "scripts" / script.name)
        self.bin = Path(directory) / "bin"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text(f"#!{sys.executable}\n{FAKE_DOCKER}")
        docker.chmod(0o755)
        self.record = Path(directory) / "docker-calls"
        self.record.write_text("")

    def env(self, **over):
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("KRONIKA_", "STARMAP_"))
        }
        env.pop("COMPOSE_KRONIKA", None)
        env["PATH"] = f"{self.bin}{os.pathsep}{env['PATH']}"
        env["FAKE_DOCKER_RECORD"] = str(self.record)
        env["STARMAP_WAIT"] = "0"
        return {**env, **over}

    def start(self, **over):
        return subprocess.Popen(
            ["bash", str(self.root / "scripts" / "demo-check.sh"), "--starmap-only"],
            env=self.env(**over),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

    def calls(self):
        return [json.loads(line) for line in self.record.read_text().splitlines()]


def verb(call):
    """The compose subcommand of one recorded call."""
    return next(a for a in call["args"] if a in ("up", "down", "run"))


class DemoCheckStarMapTests(unittest.TestCase):
    def test_its_own_stack_is_isolated_written_to_and_removed(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            port = daemon.url.rsplit(":", 1)[1]
            process = checkout.start(KRONIKA_HOST_HTTP_PORT=port)
            output, _ = process.communicate(timeout=120)
            calls = checkout.calls()
            writes = list(daemon.writes)
        self.assertEqual(process.returncode, 0, output)
        self.assertEqual(
            [verb(c) for c in calls], ["down", "up", "run", "down"], output
        )
        compose_file = str(checkout.root / "docker" / "docker-compose.kronika.yml")
        for call in calls:
            self.assertEqual(call["args"][:3], ["compose", "-f", compose_file])
            self.assertEqual(call["args"][3:5], ["-p", PROJECT])
            # Neither the containers nor the image of a user's own stack.
            self.assertEqual(call["env"]["KRONIKA_NAME_PREFIX"], PROJECT)
            self.assertEqual(call["env"]["KRONIKA_IMAGE"], f"{PROJECT}:latest")
            self.assertEqual(call["env"]["KRONIKA_HOST_GRPC_PORT"], "24317")
        self.assertEqual([path for path, _ in writes], ["/api/risks/r_1/suppress"])
        self.assertIn("1 suppressed risks on 1 nodes", output)
        self.assertNotIn("FAIL", output)

    def test_a_daemon_that_is_not_its_own_is_only_read(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            process = checkout.start(STARMAP_URL=daemon.url)
            output, _ = process.communicate(timeout=120)
            calls = checkout.calls()
            writes = list(daemon.writes)
        self.assertEqual(process.returncode, 0, output)
        self.assertEqual(calls, [])
        self.assertEqual(writes, [])
        self.assertIn("suppressed risk not checked", output)
        self.assertNotIn("FAIL", output)

    def test_an_interrupted_check_still_removes_its_stack(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Checkout(directory)
            process = checkout.start(FAKE_UP_SECONDS="3")
            deadline = time.monotonic() + 30
            while "up" not in [verb(c) for c in checkout.calls()]:
                self.assertLess(
                    time.monotonic(), deadline, "the stack was never started"
                )
                time.sleep(0.05)
            process.send_signal(signal.SIGTERM)
            output, _ = process.communicate(timeout=60)
            calls = checkout.calls()
        self.assertEqual(process.returncode, 143, output)
        self.assertEqual([verb(c) for c in calls], ["down", "up", "down"], output)


if __name__ == "__main__":
    unittest.main()

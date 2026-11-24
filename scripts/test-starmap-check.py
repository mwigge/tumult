#!/usr/bin/env python3
"""Hermetic checks of scripts/starmap-check.py against a fake daemon, and of
scripts/demo-check.sh (the sweep, the examples, the demo GameDay, the seed
and the Star Map part) against a fake `docker`."""

import fcntl
import json
import os
import re
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
JOURNAL_CHECK = Path(__file__).with_name("check-proof-journals.py")
COMPOSE_FILE = Path(__file__).parent.parent / "docker" / "docker-compose.kronika.yml"
PROJECT = "tumult-demo-check-kronika"
# Records each call (arguments and the Kronika variables) and plays the
# compose subcommands demo-check.sh uses. The seed leaves the journals of
# FAKE_JOURNALS/seed in the directory mounted at /tmp/journals. `exec cat`
# of a journal prints the one of that name in the directory of FAKE_JOURNALS
# that stands for its directory in the container: sweep for /journals, where
# the sweep and the examples write, and gameday for the directory the demo
# GameDay runs in. From its second time on it prints the one with `.again`
# after its name, if there is one. A run in the demo exits FAKE_RUN_EXIT.
FAKE_DOCKER = r"""
import json, os, shutil, sys, time
from pathlib import Path
args = sys.argv[1:]
in_container = {"/journals": "sweep", "/tmp/demo-check-gameday": "gameday"}
env = {k: v for k, v in os.environ.items() if k.startswith("KRONIKA_")}
with open(os.environ["FAKE_DOCKER_RECORD"], "a") as out:
    out.write(json.dumps({"args": args, "env": env}) + "\n")
journals = Path(os.environ["FAKE_JOURNALS"])
if "up" in args:
    time.sleep(float(os.environ.get("FAKE_UP_SECONDS", "0")))
if "run" in args and "exec" not in args:
    mount = next(a for a in args if a.endswith(":/tmp/journals"))
    for journal in (journals / "seed").iterdir():
        shutil.copy(journal, mount.rsplit(":", 1)[0])
if "exec" in args and "cat" in args:
    path = Path(args[-1])
    if str(path.parent) not in in_container:
        sys.exit(1)
    wanted = journals / in_container[str(path.parent)] / path.name
    again = wanted.with_name(wanted.name + ".again")
    with open(os.environ["FAKE_DOCKER_RECORD"]) as record:
        cats = [line for line in record if json.loads(line)["args"][-2:] == args[-2:]]
    if len(cats) > 1 and again.is_file():
        wanted = again
    if not wanted.is_file():
        sys.exit(1)
    sys.stdout.write(wanted.read_text())
if "exec" in args and "run" in args:
    print("Status: " + os.environ.get("FAKE_RUN_EXIT", "0"))
    sys.exit(int(os.environ.get("FAKE_RUN_EXIT", "0")))
if "exec" in args and "tumult gameday run" in args[-1]:
    print("GameDay score: 0.81\n  Recovery:         0.75")
    sys.exit(int(os.environ.get("FAKE_RUN_EXIT", "0")))
if "logs" in args:
    print("ResourceSpans")
"""

COMPLETED = {
    "status": "completed",
    "hypothesis_before": True,
    "hypothesis_after": True,
    "rollbacks": 0,
    "objectives": {},
}
DEVIATED = {**COMPLETED, "status": "deviated", "hypothesis_after": False}
HALTED = {**COMPLETED, "status": "halted", "hypothesis_after": None, "rollbacks": 1}
SEED_EXPECTATIONS = {
    "one.toon": {"proof": "kronika-seed", "expect": COMPLETED},
    "two.toon": {"proof": "kronika-seed", "expect": DEVIATED},
}
SWEEP_EXPECTATIONS = {
    "demo-net.toon": {"proof": "demo-check", "expect": COMPLETED},
    "demo-guard-halt.toon": {"proof": "demo-check", "expect": HALTED},
    "demo-elsewhere.toon": {"proof": "validate-only", "reason": "not in the demo"},
}
EXAMPLE_EXPECTATIONS = {
    "pg-failover.toon": {"proof": "demo-check", "expect": COMPLETED},
    "pg-load.toon": {"proof": "examples-proof", "expect": COMPLETED},
    "k8s-zone.toon": {"proof": "validate-only", "reason": "needs a cluster"},
}
# demo-check.sh runs the demo GameDay by this name.
GAMEDAY = "platform-resilience.gameday.toon"
GAMEDAY_STEPS = {
    "freeze.toon": {"proof": "demo-check", "within": GAMEDAY, "expect": COMPLETED},
    "partition.toon": {"proof": "demo-check", "within": GAMEDAY, "expect": DEVIATED},
}
GAMEDAY_SCORE = {"status": "PARTIAL", "recovery_compliance": 0.75}


def journal(expect):
    """The lines of a journal that the journal check reads."""
    status, rollbacks = expect["status"], expect["rollbacks"]
    after = expect["hypothesis_after"]
    after_block = " null" if after is None else f"\n  met: {json.dumps(after)}"
    undo = "  - name: undo\n    status: succeeded\n" * rollbacks
    return (
        f"status: {status}\n"
        "steady_state_before:\n  met: true\n"
        f"steady_state_after:{after_block}\n"
        f"rollback_results[{rollbacks}]:\n{undo}"
        "rollback_failures: 0\n"
    )


def record(expect):
    """A journal as one record of a GameDay journal."""
    first, *rest = journal(expect).splitlines()
    return "".join([f"  - {first}\n", *(f"    {line}\n" for line in rest)])


def gameday_definition(steps):
    """A GameDay file that runs the steps in order."""
    paths = "".join(f"  - path: {name}\n" for name in steps)
    return f"title: Platform resilience\nexperiments[{len(steps)}]:\n{paths}"


def gameday_journal(steps, score):
    """The lines of a GameDay journal: a record of each step, and the score."""
    records = "".join(record(step["expect"]) for step in steps.values())
    return (
        "title: Platform resilience\n"
        f"experiment_journals[{len(steps)}]:\n{records}"
        "load_result: null\n"
        "resilience_score:\n  pass_rate: 0.5\n"
        f"  recovery_compliance: {score['recovery_compliance']}\n"
        "  overall: 0.8125\n"
        f"compliance_status: {score['status']}\n"
    )


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


def compose_services(text):
    """The `image` and `container_name` values of each service, by name."""
    services, name, inside = {}, None, False
    for line in text.splitlines():
        if re.match(r"\S", line):
            inside = line.startswith("services:")
            continue
        header = re.match(r"  ([\w-]+):\s*$", line)
        if inside and header:
            name = header.group(1)
            services[name] = {"image": [], "container_name": []}
            continue
        value = re.match(r"\s+(image|container_name):\s*(.*?)\s*$", line)
        if not value:
            continue
        if not inside or name is None:
            raise AssertionError(f"outside any service: {line.strip()}")
        services[name][value.group(1)].append(value.group(2))
    return services


class KronikaComposeTests(unittest.TestCase):
    """demo-check.sh keeps its stack apart from a user's Krönika demo by
    setting KRONIKA_NAME_PREFIX and KRONIKA_IMAGE. That holds only while
    every name that is global to the Docker host is built from them."""

    def test_every_container_name_and_image_follows_the_variables(self):
        services = compose_services(COMPOSE_FILE.read_text())
        self.assertEqual(sorted(services), ["forecastd", "report", "seed", "tumultd"])
        # The opt-in forecast sidecar is built from its own pinned image.
        images = {"forecastd": "tumult-forecastd:0.1.0"}
        for name, found in services.items():
            self.assertEqual(
                found["container_name"],
                ["${KRONIKA_NAME_PREFIX:-tumult-kronika}-" + name],
                name,
            )
            self.assertEqual(
                found["image"],
                [images.get(name, "${KRONIKA_IMAGE:-tumult-kronika:latest}")],
                name,
            )

    def test_the_seed_lets_the_host_read_each_journal(self):
        # tumult writes a journal for its owner alone, and the seed user is
        # not the user that mounted /tmp/journals.
        self.assertIn(
            'chmod a+r "/tmp/journals/$$n.journal.toon"', COMPOSE_FILE.read_text()
        )

    def test_a_fixed_name_would_be_found(self):
        text = COMPOSE_FILE.read_text()
        fixed_name = text.replace(
            "${KRONIKA_NAME_PREFIX:-tumult-kronika}-seed", "tumult-kronika-seed"
        )
        self.assertEqual(
            compose_services(fixed_name)["seed"]["container_name"],
            ["tumult-kronika-seed"],
        )
        fixed_image = text.replace(
            "image: ${KRONIKA_IMAGE:-tumult-kronika:latest}",
            "image: tumult-kronika:latest",
            1,
        )
        self.assertEqual(
            compose_services(fixed_image)["tumultd"]["image"],
            ["tumult-kronika:latest"],
        )


class Checkout:
    """A copy of the scripts under a path with a space, and a fake `docker`."""

    def __init__(self, directory):
        self.root = Path(directory) / "a checkout"
        (self.root / "scripts").mkdir(parents=True)
        (self.root / "docker").mkdir()
        (self.root / "docker" / "docker-compose.kronika.yml").write_text(
            "services: {}\n"
        )
        for script in (DEMO_CHECK, SCRIPT, JOURNAL_CHECK):
            shutil.copy(script, self.root / "scripts" / script.name)
        # What the fake docker hands out: journals that are as expected.
        self.journals = Path(directory) / "journals"
        self.expect("demo/kronika/experiments", "seed", SEED_EXPECTATIONS)
        self.expect("demo/experiments", "sweep", SWEEP_EXPECTATIONS)
        # The examples write their journals to /journals, as the sweep does.
        self.expect("examples", "sweep", EXAMPLE_EXPECTATIONS)
        self.expect_gameday(GAMEDAY_STEPS, GAMEDAY_SCORE)
        self.bin = Path(directory) / "bin"
        self.bin.mkdir()
        docker = self.bin / "docker"
        docker.write_text(f"#!{sys.executable}\n{FAKE_DOCKER}")
        docker.chmod(0o755)
        self.record = Path(directory) / "docker-calls"
        self.record.write_text("")
        # Where demo-check.sh keeps its lock: never the host's own /tmp.
        self.tmp = Path(directory) / "tmp"
        self.tmp.mkdir()
        self.lock = self.tmp / f"{PROJECT}.lock"

    def expect(self, directory, kind, experiments):
        """Write an expectations file, and a journal for each `expect` in it."""
        (self.root / directory).mkdir(parents=True)
        (self.root / directory / "expectations.json").write_text(
            json.dumps({"experiments": experiments})
        )
        (self.journals / kind).mkdir(parents=True, exist_ok=True)
        for name, entry in experiments.items():
            if "expect" in entry:
                self.journal(kind, name).write_text(journal(entry["expect"]))

    def expect_gameday(self, steps, score):
        """Write the expectations and the file of the demo GameDay, and the
        journal of its run."""
        directory = self.root / "demo" / "kronika" / "gamedays"
        directory.mkdir(parents=True)
        expect = {"load_result": False, "score": score}
        gamedays = {GAMEDAY: {"proof": "demo-check", "expect": expect}}
        (directory / "expectations.json").write_text(
            json.dumps({"experiments": steps, "gamedays": gamedays})
        )
        (directory / GAMEDAY).write_text(gameday_definition(steps))
        (self.journals / "gameday").mkdir()
        self.journal("gameday", GAMEDAY).write_text(gameday_journal(steps, score))

    def journal(self, kind, name):
        """Where the fake docker keeps the journal of one experiment."""
        stem = name.removesuffix(".toon")
        return self.journals / kind / f"{stem}.journal.toon"

    def replace_checker(self, body):
        """Stand a script that only does `body` in for starmap-check.py."""
        (self.root / "scripts" / SCRIPT.name).write_text(body)

    def env(self, **over):
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(("KRONIKA_", "STARMAP_"))
        }
        env.pop("COMPOSE_KRONIKA", None)
        env["PATH"] = f"{self.bin}{os.pathsep}{env['PATH']}"
        env["FAKE_DOCKER_RECORD"] = str(self.record)
        env["FAKE_JOURNALS"] = str(self.journals)
        env["STARMAP_WAIT"] = "0"
        env["TMPDIR"] = str(self.tmp)
        return {**env, **over}

    def start(self, *arguments, **over):
        arguments = arguments or ("--starmap-only",)
        return subprocess.Popen(
            ["bash", str(self.root / "scripts" / "demo-check.sh"), *arguments],
            env=self.env(**over),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

    def calls(self):
        return [json.loads(line) for line in self.record.read_text().splitlines()]

    def left_behind(self):
        """The directories demo-check.sh made for itself and did not remove."""
        return [path for path in self.tmp.iterdir() if path.is_dir()]

    def sweep(self, daemon, mode="full", **over):
        """Run the sweep against the fake docker; the Star Map is only read."""
        process = self.start(
            "--mode",
            mode,
            "--no-wait",
            COMPOSE_DEMO="docker compose -f demo.yml",
            COLLECTOR_METRICS_URL=f"{daemon.url}/metrics",
            STARMAP_URL=daemon.url,
            **over,
        )
        output, _ = process.communicate(timeout=120)
        return process.returncode, output

    def runs(self, directory="/demo/experiments"):
        """The experiment of each `tumult run` of `directory` in the demo, in
        order."""
        ran = [c["args"] for c in self.calls() if "tumult" in c["args"]]
        paths = [args[args.index("run") + 1] for args in ran]
        return [path for path in paths if path.startswith(f"{directory}/")]


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
            # The daemon refuses to start on a bootstrap token of another shape.
            self.assertRegex(call["env"]["KRONIKA_BOOTSTRAP_TOKEN"], r"^kro_.{20,}$")
            self.assertGreaterEqual(
                len(call["env"]["KRONIKA_BOOTSTRAP_ADMIN_PASSWORD"]), 12
            )
        self.assertEqual([path for path, _ in writes], ["/api/risks/r_1/suppress"])
        self.assertIn("1 suppressed risks on 1 nodes", output)
        self.assertNotIn("FAIL", output)
        # The seed is judged by its journals, one line for each.
        self.assertRegex(output, r"PASS.*seed — one\.toon")
        self.assertRegex(output, r"PASS.*seed — two\.toon")

    def test_the_journals_of_the_seed_are_in_a_directory_that_is_removed(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            port = daemon.url.rsplit(":", 1)[1]
            process = checkout.start(KRONIKA_HOST_HTTP_PORT=port)
            output, _ = process.communicate(timeout=120)
            seed = next(c["args"] for c in checkout.calls() if verb(c) == "run")
            left = checkout.left_behind()
        self.assertEqual(process.returncode, 0, output)
        mount = seed[seed.index("-v") + 1]
        self.assertTrue(mount.startswith(f"{checkout.tmp}/"), mount)
        self.assertTrue(mount.endswith(":/tmp/journals"), mount)
        self.assertLess(seed.index("-v"), seed.index("seed"))
        self.assertEqual(left, [])

    def test_a_seed_journal_that_is_not_as_expected_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.journal("seed", "two.toon").write_text(journal(COMPLETED))
            port = daemon.url.rsplit(":", 1)[1]
            process = checkout.start(KRONIKA_HOST_HTTP_PORT=port)
            output, _ = process.communicate(timeout=120)
            calls = checkout.calls()
        self.assertEqual(process.returncode, 1, output)
        self.assertRegex(output, r"PASS.*seed — one\.toon")
        self.assertRegex(output, r"FAIL.*seed — two\.toon")
        self.assertIn("status: expected deviated, journal has completed", output)
        self.assertNotIn("suppressed risks", output, "the map was not checked")
        self.assertEqual([verb(c) for c in calls], ["down", "up", "run", "down"])

    def test_a_seed_that_leaves_no_journal_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.journal("seed", "one.toon").unlink()
            port = daemon.url.rsplit(":", 1)[1]
            process = checkout.start(KRONIKA_HOST_HTTP_PORT=port)
            output, _ = process.communicate(timeout=120)
        self.assertEqual(process.returncode, 1, output)
        self.assertIn("no journal one.journal.toon", output)

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
            left = checkout.left_behind()
        self.assertEqual(process.returncode, 143, output)
        self.assertEqual([verb(c) for c in calls], ["down", "up", "down"], output)
        self.assertEqual(left, [])

    def test_two_checks_at_once_take_turns(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            port = daemon.url.rsplit(":", 1)[1]
            both = [
                checkout.start(KRONIKA_HOST_HTTP_PORT=port, FAKE_UP_SECONDS="1")
                for _ in range(2)
            ]
            outputs = [process.communicate(timeout=120)[0] for process in both]
            calls = checkout.calls()
        for process, output in zip(both, outputs):
            self.assertEqual(process.returncode, 0, output)
            self.assertNotIn("FAIL", output)
        # Neither removed the stack the other was using.
        self.assertEqual(
            [verb(c) for c in calls],
            ["down", "up", "run", "down"] * 2,
            "\n".join(outputs),
        )
        self.assertEqual(
            sum("waiting for another demo check" in o for o in outputs), 1, outputs
        )

    def test_a_check_that_cannot_have_its_turn_touches_nothing(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            port = daemon.url.rsplit(":", 1)[1]
            with open(checkout.lock, "w") as held:
                fcntl.flock(held, fcntl.LOCK_EX)
                process = checkout.start(
                    KRONIKA_HOST_HTTP_PORT=port, STARMAP_LOCK_WAIT="0"
                )
                output, _ = process.communicate(timeout=120)
            calls = checkout.calls()
        self.assertEqual(process.returncode, 1, output)
        self.assertEqual(calls, [], "the other check's stack is left alone")
        self.assertIn("another demo check still holds", output)
        self.assertIn(str(checkout.lock), output)

    def test_a_check_interrupted_while_it_waits_ends_at_once(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Checkout(directory)
            with open(checkout.lock, "w") as held:
                fcntl.flock(held, fcntl.LOCK_EX)
                process = checkout.start(STARMAP_LOCK_WAIT="60")
                waiting = process.stdout.readline()
                while waiting and "waiting for another demo check" not in waiting:
                    waiting = process.stdout.readline()
                self.assertTrue(waiting, "the check never said it was waiting")
                asked = time.monotonic()
                process.send_signal(signal.SIGTERM)
                output, _ = process.communicate(timeout=30)
                took = time.monotonic() - asked
            calls = checkout.calls()
        self.assertEqual(process.returncode, 143, output)
        self.assertLess(took, 10, "it did not wait out the lock")
        self.assertEqual(calls, [], "the other check's stack is left alone")

    def test_a_lock_file_that_cannot_be_opened_is_one_clear_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Checkout(directory)
            nowhere = str(checkout.tmp / "not there")
            process = checkout.start(TMPDIR=nowhere)
            output, _ = process.communicate(timeout=120)
            calls = checkout.calls()
        self.assertEqual(process.returncode, 1, output)
        self.assertEqual(calls, [])
        self.assertIn("cannot open the lock file", output)
        self.assertIn("1 check(s) failed", output)

    def test_a_lock_file_nobody_holds_is_no_obstacle(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.lock.write_text("")
            port = daemon.url.rsplit(":", 1)[1]
            process = checkout.start(KRONIKA_HOST_HTTP_PORT=port, STARMAP_LOCK_WAIT="0")
            output, _ = process.communicate(timeout=120)
        self.assertEqual(process.returncode, 0, output)
        self.assertNotIn("waiting", output)

    def test_without_docker_there_is_one_clear_message(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Checkout(directory)
            process = checkout.start(COMPOSE_KRONIKA="no-such-docker compose -f x")
            output, _ = process.communicate(timeout=120)
            calls = checkout.calls()
        self.assertEqual(process.returncode, 1, output)
        self.assertEqual(calls, [])
        told = [line for line in output.splitlines() if "no-such-docker" in line]
        self.assertEqual(len(told), 1, output)
        self.assertIn("is not installed or not on PATH", told[0])
        self.assertIn("STARMAP_URL", told[0])
        self.assertNotIn("WARN", output)
        self.assertNotIn("did not come up", output)

    def test_a_running_daemon_is_checked_without_docker(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            process = checkout.start(
                COMPOSE_KRONIKA="no-such-docker compose -f x", STARMAP_URL=daemon.url
            )
            output, _ = process.communicate(timeout=120)
        self.assertEqual(process.returncode, 0, output)

    def test_a_check_that_prints_nothing_is_a_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Checkout(directory)
            checkout.replace_checker("")
            process = checkout.start(STARMAP_URL="http://127.0.0.1:1")
            output, _ = process.communicate(timeout=120)
        self.assertEqual(process.returncode, 1, output)
        self.assertIn("the check printed nothing", output)
        self.assertIn("1 check(s) failed", output)

    def test_a_failed_line_fails_the_check_whatever_the_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Checkout(directory)
            checkout.replace_checker(
                'print("ok   star map: one")\nprint("FAIL star map: two")\n'
            )
            process = checkout.start(STARMAP_URL="http://127.0.0.1:1")
            output, _ = process.communicate(timeout=120)
        self.assertEqual(process.returncode, 1, output)
        self.assertIn("star map: two", output)
        self.assertIn("1 check(s) failed", output)


class DemoCheckSweepTests(unittest.TestCase):
    def test_the_sweep_runs_what_the_expectations_give_this_proof(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            status, output = checkout.sweep(daemon)
            runs = checkout.runs()
            left = checkout.left_behind()
        self.assertEqual(status, 0, output)
        self.assertEqual(
            runs,
            [
                "/demo/experiments/demo-net.toon",
                "/demo/experiments/demo-guard-halt.toon",
                "/demo/experiments/demo-net.toon",
            ],
        )
        self.assertRegex(output, r"PASS.*demo-net\.toon")
        self.assertRegex(output, r"PASS.*demo-guard-halt\.toon")
        self.assertNotIn("FAIL", output)
        self.assertEqual(left, [])

    def test_the_first_runs_again_in_the_same_stack_and_that_run_is_judged(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            first = checkout.journal("sweep", "demo-net.toon")
            failed = journal({**COMPLETED, "status": "failed"})
            first.with_name(first.name + ".again").write_text(failed)
            status, output = checkout.sweep(daemon)
        self.assertEqual(status, 1, output)
        self.assertRegex(output, r"FAIL.*demo-net\.toon")
        self.assertIn("status: expected completed, journal has failed", output)

    def test_a_run_is_judged_by_its_journal_not_by_its_exit_status(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            status, output = checkout.sweep(daemon, FAKE_RUN_EXIT="1")
        self.assertEqual(status, 0, output)
        self.assertNotIn("FAIL", output)

    def test_a_journal_that_is_not_as_expected_fails_and_is_kept(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            failed = journal({**COMPLETED, "status": "failed"})
            checkout.journal("sweep", "demo-net.toon").write_text(failed)
            status, output = checkout.sweep(daemon)
            left = checkout.left_behind()
            kept = [path.name for path in left[0].glob("sweep/*")] if left else []
        self.assertEqual(status, 1, output)
        self.assertRegex(output, r"FAIL.*demo-net\.toon")
        self.assertIn("status: expected completed, journal has failed", output)
        self.assertRegex(output, r"PASS.*demo-guard-halt\.toon")
        self.assertIn("1 check(s) failed", output)
        self.assertEqual(len(left), 1, output)
        self.assertIn(f"kept in {left[0]}", output)
        self.assertIn("demo-net.journal.toon", kept)
        self.assertIn("demo-net.log", kept)

    def test_a_journal_of_an_earlier_run_is_not_judged(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            status, output = checkout.sweep(daemon)
            calls = [c["args"] for c in checkout.calls() if "exec" in c["args"]]
        self.assertEqual(status, 0, output)
        journal_path = "/journals/demo-net.journal.toon"
        removed = next(
            i for i, a in enumerate(calls) if a[-3:] == ["rm", "-f", journal_path]
        )
        ran = next(
            i for i, a in enumerate(calls) if "tumult" in a and journal_path in a
        )
        self.assertLess(removed, ran)

    def test_a_run_that_leaves_no_journal_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.journal("sweep", "demo-net.toon").unlink()
            status, output = checkout.sweep(daemon)
        self.assertEqual(status, 1, output)
        self.assertIn("no journal demo-net.journal.toon", output)
        self.assertIn("1 check(s) failed", output)

    def test_no_expectations_file_is_one_clear_failure(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            (checkout.root / "demo/experiments/expectations.json").unlink()
            status, output = checkout.sweep(daemon)
            runs = checkout.runs()
        self.assertEqual(status, 1, output)
        self.assertEqual(runs, [])
        self.assertIn("sweep — nothing to run", output)

    def test_the_warm_up_leaves_out_the_halting_run_and_never_fails(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.journal("sweep", "demo-net.toon").unlink()
            status, output = checkout.sweep(daemon, mode="populate", FAKE_RUN_EXIT="1")
            runs = checkout.runs()
            left = checkout.left_behind()
        self.assertEqual(status, 0, output)
        self.assertEqual(runs, ["/demo/experiments/demo-net.toon"])
        self.assertNotIn("FAIL", output)
        self.assertEqual(left, [])


STANDBY_OUT = ["rm", "-sfv", "demo-postgres-standby"]
PAIR_UP = ["up", "-d", "--wait", "demo-postgres", "demo-postgres-standby"]


def example_steps(calls):
    """What the examples step did in the demo, in order: each reset of the
    PostgreSQL pair, and each run of an example."""
    steps = []
    for args in (call["args"] for call in calls):
        ran = args[args.index("run") + 1] if "tumult" in args else ""
        if args[3:] in (STANDBY_OUT, PAIR_UP):
            steps.append(" ".join(args[3:]))
        elif ran.startswith("/demo/examples/"):
            steps.append(ran)
    return steps


def kept_files(left, part):
    """The names of the files of `part` in the one directory that was kept."""
    return sorted(path.name for path in left[0].glob(f"{part}/*")) if left else []


class DemoCheckExamplesTests(unittest.TestCase):
    def test_the_examples_of_this_proof_run_each_on_a_fresh_pair(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            status, output = checkout.sweep(daemon)
            steps = example_steps(checkout.calls())
            left = checkout.left_behind()
        self.assertEqual(status, 0, output)
        reset = [" ".join(STANDBY_OUT), " ".join(PAIR_UP)]
        self.assertEqual(
            steps, [*reset, "/demo/examples/pg-failover.toon", *reset], output
        )
        self.assertRegex(output, r"PASS.*example: pg-failover\.toon")
        self.assertNotIn("FAIL", output)
        self.assertEqual(left, [])

    def test_an_example_journal_that_is_not_as_expected_fails_and_is_kept(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.journal("sweep", "pg-failover.toon").write_text(journal(DEVIATED))
            status, output = checkout.sweep(daemon)
            left = checkout.left_behind()
            kept = kept_files(left, "examples")
        self.assertEqual(status, 1, output)
        self.assertRegex(output, r"FAIL.*example: pg-failover\.toon")
        self.assertIn("status: expected completed, journal has deviated", output)
        self.assertIn("1 check(s) failed", output)
        self.assertEqual(len(left), 1, output)
        self.assertIn(f"kept in {left[0]}", output)
        self.assertIn("pg-failover.journal.toon", kept)
        self.assertIn("pg-failover.log", kept)

    def test_an_example_that_leaves_no_journal_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.journal("sweep", "pg-failover.toon").unlink()
            status, output = checkout.sweep(daemon)
        self.assertEqual(status, 1, output)
        self.assertRegex(output, r"FAIL.*example: pg-failover\.toon")
        self.assertIn("no journal pg-failover.journal.toon", output)
        self.assertIn("1 check(s) failed", output)


class DemoCheckGameDayTests(unittest.TestCase):
    def test_the_demo_gameday_runs_in_the_demo_and_its_journal_is_judged(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            status, output = checkout.sweep(daemon)
            shells = [c["args"][-1] for c in checkout.calls() if "sh" in c["args"]]
            left = checkout.left_behind()
        self.assertEqual(status, 0, output)
        self.assertEqual(len(shells), 1, shells)
        self.assertIn(
            f"tumult gameday run /tmp/demo-check-gameday/{GAMEDAY}", shells[0]
        )
        self.assertIn("GameDay score 0.81, 0.75 recovery", output)
        for line in ("", ": freeze.toon", ": partition.toon"):
            self.assertRegex(
                output, rf"(?m)PASS.*gameday: {re.escape(GAMEDAY + line)}$"
            )
        self.assertNotIn("FAIL", output)
        self.assertEqual(left, [])

    def test_a_gameday_journal_that_is_not_as_expected_fails_and_is_kept(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            compliant = {**GAMEDAY_SCORE, "status": "COMPLIANT"}
            checkout.journal("gameday", GAMEDAY).write_text(
                gameday_journal(GAMEDAY_STEPS, compliant)
            )
            status, output = checkout.sweep(daemon)
            left = checkout.left_behind()
            kept = kept_files(left, "gameday")
        self.assertEqual(status, 1, output)
        self.assertRegex(output, rf"(?m)FAIL.*gameday: {re.escape(GAMEDAY)}$")
        self.assertIn("score status: expected PARTIAL, journal has COMPLIANT", output)
        self.assertIn("1 check(s) failed", output)
        self.assertEqual(len(left), 1, output)
        self.assertIn(f"kept in {left[0]}", output)
        self.assertIn("platform-resilience.gameday.journal.toon", kept)
        self.assertIn("platform-resilience.log", kept)

    def test_a_gameday_that_leaves_no_journal_fails_the_check(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            checkout = Checkout(directory)
            checkout.journal("gameday", GAMEDAY).unlink()
            status, output = checkout.sweep(daemon)
        self.assertEqual(status, 1, output)
        self.assertRegex(output, rf"(?m)FAIL.*gameday: {re.escape(GAMEDAY)}$")
        self.assertIn("no journal platform-resilience.gameday.journal.toon", output)
        self.assertIn("1 check(s) failed", output)


if __name__ == "__main__":
    unittest.main()

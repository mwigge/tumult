#!/usr/bin/env python3
"""Hermetic checks of scripts/dr-check.py against a fake daemon.

The fake daemon gives the answers a Krönika daemon gave in `make demo-check`
after the DR examples reported to it, recorded in testdata/dr-check with the
journals of those runs. The answer of `GET /api/dora` is cut to its
`scenarios` block, the reports to their markup without the style sheet, and
the route module of the DR page to its first line and the template that
holds the heading: the parts the check reads.
"""

import copy
import json
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

SCRIPT = Path(__file__).with_name("dr-check.py")
DATA = Path(__file__).with_name("testdata") / "dr-check"
TOKEN = "kro_0123456789abcdef0123456789abcdef"
FUNCTION = "cif-0001"

RPO_ID = "8196e02c-07b2-44de-b0d6-1ba4ff2fa224"
RPO_TITLE = "PostgreSQL restore from the last backup"
FAILOVER_ID = "53fe0625-dba5-4e5f-a88d-b90ecfe38d04"
LOAD_ID = "0f6c3a2e-8d47-4b1a-a1c4-5d0e9b7f3a12"
REPORT_ID = "KRK-R4-20261124-cd7d8b"
PACK_ID = "KRK-R2-20261124-7076b6"
MODULE_PATH = "/_app/immutable/nodes/7.2QGJf5gI.js"

EXPECTED = {"status": "completed", "objectives": {"rto": "met"}}
EXPECTATIONS = {
    "experiments": {
        "postgres-failover.toon": {"proof": "demo-check", "expect": EXPECTED},
        "postgres-restore-rpo.toon": {
            "proof": "demo-check",
            "expect": {**EXPECTED, "rpo_measured_s": {"from": 3, "to": 10}},
        },
        "pg-under-load.toon": {"proof": "examples-proof", "expect": EXPECTED},
        "k8s-zone-failover.toon": {"proof": "validate-only", "reason": "a cluster"},
    }
}


def recorded(name):
    return (DATA / name).read_text()


def recorded_json(name):
    return json.loads(recorded(name))


DR_VIEW = recorded_json("api-dr.json")
DORA = recorded_json("api-dora.json")
REPORT = recorded("dr-test.html")
PACK = recorded("evidence-pack.html")


def replaced(text, old, new):
    """`text` with its one `old` replaced: a recorded answer changed for a case."""
    assert text.count(old) == 1, old
    return text.replace(old, new)


def dr_view(drop=None, run=None, **over):
    """The recorded DR view without the test of the run `drop`, and with the
    test of the run `run` changed by `over`."""
    view = copy.deepcopy(DR_VIEW)
    view["tests"] = [t for t in view["tests"] if t["evidence_id"] != drop]
    for test in view["tests"]:
        if test["evidence_id"] == run:
            test.update(over)
    return view


def dora(**cell):
    """The recorded DORA score with the backup_restore cell of the function
    changed by `cell`."""
    doc = copy.deepcopy(DORA)
    functions = doc["scenarios"]["functions"]
    function = next(f for f in functions if f["entity_id"] == FUNCTION)
    backup = next(s for s in function["scenarios"] if s["id"] == "backup_restore")
    backup.update(cell)
    return doc


def answer(body, status=200, kind="application/json"):
    """What the fake daemon sends: a status, a media type and a body."""
    text = body if isinstance(body, str) else json.dumps(body)
    return status, kind, text.encode()


def html(text, status=200):
    return answer(text, status, "text/html")


def answers(**over):
    """The answers of the daemon as recorded."""
    base = {
        "dr": [answer(DR_VIEW)],
        "dora": [answer(DORA)],
        "/dr": html(recorded("dr-shell.html")),
        "/dr.html": html(recorded("dr.html")),
        MODULE_PATH: answer(recorded("dr-route.js"), kind="text/javascript"),
        "dr-test": answer(recorded_json("dr-test.meta.json")),
        "evidence-pack": answer(recorded_json("evidence-pack.meta.json")),
        REPORT_ID: html(REPORT),
        PACK_ID: html(PACK),
    }
    return {**base, **over}


class FakeDaemon:
    """Serves recorded answers and keeps each request it was sent."""

    def __init__(self, **over):
        self.answers = answers(**over)
        self.requests = []
        daemon = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def send(self, reply):
                status, kind, body = reply
                self.send_response(status)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def keep(self, body=None):
                daemon.requests.append(
                    {
                        "method": self.command,
                        "path": self.path,
                        "auth": self.headers.get("Authorization"),
                        "body": body,
                    }
                )

            def do_GET(self):
                self.keep()
                self.send(daemon.reply_to(self.path))

            def do_POST(self):
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length))
                self.keep(body)
                self.send(daemon.answers.get(body.get("type"), answer({}, 404)))

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def reply_to(self, path):
        """The answer of a GET: the next one of a read that can change."""
        route = urlsplit(path).path
        turns = {"/api/dr": "dr", "/api/dora": "dora"}
        if route in turns:
            replies = self.answers[turns[route]]
            return replies.pop(0) if len(replies) > 1 else replies[0]
        if route.startswith("/api/reports/v2/") and route.endswith("/html"):
            return self.answers.get(route.split("/")[4], html("report not found", 404))
        return self.answers.get(route, html("not found", 404))

    def reads(self, route):
        return [r for r in self.requests if urlsplit(r["path"]).path == route]

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


class Examples:
    """The expectations of the examples and the journals of their runs."""

    def __init__(self, directory):
        self.root = Path(directory)
        self.expectations = self.root / "expectations.json"
        self.expectations.write_text(json.dumps(EXPECTATIONS))
        self.journals = self.root / "journals"
        self.journals.mkdir()
        for stem in ("postgres-failover", "postgres-restore-rpo"):
            self.write(stem, recorded(f"{stem}.journal.toon"))
        # A run of another proof is never asked about.
        failover = recorded("postgres-failover.journal.toon")
        self.write("pg-under-load", replaced(failover, FAILOVER_ID, LOAD_ID))

    def write(self, stem, text):
        (self.journals / f"{stem}.journal.toon").write_text(text)

    def check(self, daemon, *flags, url=None):
        """Run the check against `daemon`; its exit status and its lines."""
        done = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                url or daemon.url,
                "--expectations",
                str(self.expectations),
                "--journals",
                str(self.journals),
                "--function",
                FUNCTION,
                "--wait",
                "0",
                *flags,
            ],
            env={"PATH": "/usr/bin:/bin", "TUMULT_DAEMON_TOKEN": TOKEN},
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        return done.returncode, done.stdout.splitlines(), done.stderr


def failed(lines):
    return [line for line in lines if not line.startswith("ok   ")]


class DrCheckTests(unittest.TestCase):
    def run_check(self, *flags, **over):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon(**over) as daemon:
            examples = Examples(directory)
            status, lines, errors = examples.check(daemon, *flags)
        self.daemon = daemon
        return status, lines, errors

    def test_a_demo_that_holds_every_dr_fact_passes_each_line_with_its_numbers(self):
        status, lines, errors = self.run_check()
        self.assertEqual(status, 0, lines + [errors])
        self.assertEqual(
            lines,
            [
                "ok   dr: postgres-failover.toon is a DR test of demo-postgres in demo:"
                " verdict met, recovered true, RTO 3.2 s of 30.0 s",
                "ok   dr: postgres-restore-rpo.toon is a DR test of demo-postgres in demo:"
                " verdict met, recovered true, RTO 3.8 s of 30.0 s,"
                " RPO 6.4 s (expected 3 to 10)",
                "ok   dr page: GET /dr answered 200 with the app shell, which builds the"
                " page in the browser; its route module"
                f" {MODULE_PATH} holds the heading Disaster recovery",
                f"ok   dr report {REPORT_ID}: the row of {RPO_TITLE} reads RPO actual"
                " 6.4 s and verdict met, the run measured 6.4 s, and its evidence"
                f" names run {RPO_ID}",
                f"ok   evidence pack {PACK_ID}: its DORA matrix holds the row"
                " RTS 2024/1774 Art. 26: not applicable, the profile of the legal"
                " entity excludes this row",
                f"ok   dora: backup_restore of {FUNCTION} (Order processing) is passed,"
                f" last tested by run {RPO_ID} of postgres-restore-rpo.toon",
            ],
        )

    def test_the_daemon_is_asked_with_the_token_for_the_target_of_each_run(self):
        self.run_check()
        reads = self.daemon.reads("/api/dr")
        self.assertEqual(len(reads), 1, "one read for the one target")
        query = parse_qs(urlsplit(reads[0]["path"]).query)
        self.assertEqual(query, {"target": ["demo-postgres"], "environment": ["demo"]})
        posts = [r for r in self.daemon.requests if r["method"] == "POST"]
        self.assertEqual(
            [r["body"] for r in posts],
            [
                {"type": "dr-test", "target": "demo-postgres", "environment": "demo"},
                {"type": "evidence-pack", "framework": "dora"},
            ],
        )
        api = [r for r in self.daemon.requests if r["path"].startswith("/api/")]
        self.assertEqual({r["auth"] for r in api}, {f"Bearer {TOKEN}"})
        # The page is read as a browser reads it, with no token.
        pages = [r for r in self.daemon.requests if not r["path"].startswith("/api/")]
        self.assertEqual([r["path"] for r in pages], ["/dr", "/dr.html", MODULE_PATH])
        self.assertEqual({r["auth"] for r in pages}, {None})

    def test_a_run_the_dr_view_does_not_hold_fails_its_line(self):
        status, lines, _ = self.run_check(dr=[answer(dr_view(drop=FAILOVER_ID))])
        self.assertEqual(status, 1, lines)
        self.assertEqual(
            failed(lines),
            [
                f"FAIL dr: postgres-failover.toon: run {FAILOVER_ID} is not among the"
                " 1 DR tests of demo-postgres in demo"
            ],
        )

    def test_a_test_that_is_breached_or_not_recovered_fails(self):
        for over in ({"verdict": "breached"}, {"recovered": False}):
            with self.subTest(over=over):
                view = dr_view(run=FAILOVER_ID, **over)
                status, lines, _ = self.run_check(dr=[answer(view)])
                self.assertEqual(status, 1, lines)
                self.assertEqual(len(failed(lines)), 1, lines)
                self.assertTrue(
                    failed(lines)[0].startswith("FAIL dr: postgres-failover"), lines
                )

    def test_an_rpo_outside_the_range_of_the_expectations_fails(self):
        for rpo in (2.9, 10.2, None):
            with self.subTest(rpo=rpo):
                view = dr_view(run=RPO_ID, rpo_measured_s=rpo)
                status, lines, _ = self.run_check(dr=[answer(view)])
                self.assertEqual(status, 1, lines)
                self.assertEqual(len(failed(lines)), 1, lines)
                self.assertIn("(expected 3 to 10)", failed(lines)[0])

    def test_a_dr_view_that_is_refused_is_one_failure_with_its_status(self):
        refused = answer({"error": "unauthorized"}, 401)
        status, lines, _ = self.run_check(dr=[refused])
        self.assertEqual(status, 1, lines)
        self.assertEqual(
            failed(lines),
            ["FAIL dr: GET /api/dr?target=demo-postgres&environment=demo answered 401"],
        )

    def test_a_page_whose_route_module_lacks_the_dr_page_fails(self):
        cases = {
            "/dr": html("not found", 404),
            "/dr.html": html(recorded("dr-shell.html")),
            MODULE_PATH: answer("export{};", kind="text/javascript"),
        }
        for route, reply in cases.items():
            with self.subTest(route=route):
                status, lines, _ = self.run_check(**{route: reply})
                self.assertEqual(status, 1, lines)
                self.assertEqual(len(failed(lines)), 1, lines)
                self.assertTrue(failed(lines)[0].startswith("FAIL dr page: "), lines)

    def test_a_report_that_does_not_give_the_rpo_run_as_measured_fails(self):
        row = f"<td>{RPO_TITLE} (automated)</td>"
        glyph = "<span class='glyph' style='color:#009E73'>●</span>"
        verdict = f"<td>6.4 s</td><td>{glyph} met</td>"
        run_id = f"<tr><th>Run id</th><td>{RPO_ID}</td></tr>"
        reports = {
            "no row": replaced(REPORT, row, "<td>Another run (automated)</td>"),
            "another rpo": replaced(REPORT, "<td>6.4 s</td>", "<td>4.0 s</td>"),
            "breached": replaced(REPORT, verdict, "<td>6.4 s</td><td>breached</td>"),
            "no run id": replaced(REPORT, run_id, ""),
        }
        for case, text in reports.items():
            with self.subTest(case=case):
                status, lines, _ = self.run_check(**{REPORT_ID: html(text)})
                self.assertEqual(status, 1, lines)
                self.assertEqual(len(failed(lines)), 1, lines)
                self.assertTrue(
                    failed(lines)[0].startswith(f"FAIL dr report {REPORT_ID}: "), lines
                )

    def test_a_report_that_is_not_built_fails_with_its_status(self):
        status, lines, _ = self.run_check(**{"dr-test": answer({"error": "x"}, 500)})
        self.assertEqual(status, 1, lines)
        self.assertEqual(
            failed(lines),
            [
                "FAIL dr report of demo-postgres in demo: POST /api/reports/v2/generate"
                " answered 500"
            ],
        )

    def test_an_evidence_pack_without_the_row_of_art26_fails(self):
        cite = "<td>RTS 2024/1774 Art. 26</td>"
        pack = replaced(PACK, cite, "<td>RTS 2024/1774 Art. 26a</td>")
        status, lines, _ = self.run_check(**{PACK_ID: html(pack)})
        self.assertEqual(status, 1, lines)
        self.assertEqual(
            failed(lines),
            [
                f"FAIL evidence pack {PACK_ID}: its DORA matrix has no row"
                " RTS 2024/1774 Art. 26"
            ],
        )

    def test_a_plan_scenario_that_the_rpo_run_did_not_pass_fails(self):
        manual = {"id": "manual-1", "source": "manual", "outcome": "passed"}
        tested = {"id": RPO_ID, "source": "run", "outcome": "failed"}
        cases = {
            "missing": dora(state="missing", last_test=None),
            "failed": dora(state="failed", last_test=tested),
            "another test": dora(last_test=manual),
            "no function": {"scenarios": {"rows": [], "functions": []}},
        }
        for case, body in cases.items():
            with self.subTest(case=case):
                status, lines, _ = self.run_check(dora=[answer(body)])
                self.assertEqual(status, 1, lines)
                self.assertEqual(len(failed(lines)), 1, lines)
                self.assertTrue(failed(lines)[0].startswith("FAIL dora: "), lines)

    def test_the_dora_score_is_read_again_until_it_holds(self):
        late = [answer(dora(state="missing", last_test=None)), answer(DORA)]
        status, lines, _ = self.run_check("--wait", "10", dora=late)
        self.assertEqual(status, 0, lines)
        self.assertEqual(len(self.daemon.reads("/api/dora")), 2)

    def test_a_dr_example_without_a_journal_fails_and_another_proof_is_not_read(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            examples = Examples(directory)
            (examples.journals / "postgres-failover.journal.toon").unlink()
            status, lines, _ = examples.check(daemon)
        self.assertEqual(status, 1, lines)
        self.assertEqual(
            failed(lines), ["FAIL dr: no journal of postgres-failover.toon to read"]
        )
        self.assertNotIn(LOAD_ID, "\n".join(lines))

    def test_a_run_of_another_kind_is_no_dr_example(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            examples = Examples(directory)
            failover = recorded("postgres-failover.journal.toon")
            chaos = replaced(failover, "kind: dr", "kind: chaos")
            examples.write("postgres-failover", chaos)
            status, lines, _ = examples.check(daemon)
        self.assertEqual(status, 0, lines)
        self.assertFalse(any("postgres-failover" in line for line in lines), lines)

    def test_a_daemon_that_is_not_there_fails_each_line_clearly(self):
        with tempfile.TemporaryDirectory() as directory, FakeDaemon() as daemon:
            examples = Examples(directory)
            status, lines, _ = examples.check(daemon, url="http://127.0.0.1:1")
        self.assertEqual(status, 1, lines)
        self.assertEqual(len(lines), 5, lines)
        for line in lines:
            self.assertRegex(line, r"^FAIL .*: http://127\.0\.0\.1:1 did not answer")


if __name__ == "__main__":
    unittest.main()

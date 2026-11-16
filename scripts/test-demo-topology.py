#!/usr/bin/env python3
"""Hermetic checks of scripts/demo-topology.sh against a fake `tumult`: every
proof line of the script can fail."""

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("demo-topology.sh")
# Says what the test wants each subcommand to say. A run writes its journal as
# the CLI does (never over one that exists, unless forced), notes the store it
# was given in runs.log, prints its status line and ends with FAKE_RUN_EXIT.
FAKE_TUMULT = r"""#!/usr/bin/env python3
import os, sys
from pathlib import Path
args = sys.argv[1:]
said = {"map": "FAKE_MAP", "lineage": "FAKE_LINEAGE", "once": "FAKE_AUTOPILOT"}
if args[0] == "run":
    journal = Path("journal.toon")
    if "--journal-path" in args:
        journal = Path(args[args.index("--journal-path") + 1])
    if journal.exists() and "--force" not in args:
        sys.exit("journal already exists: " + str(journal))
    journal.write_text("status: " + os.environ.get("FAKE_STATUS", "Completed"))
    with open(Path(__file__).with_name("runs.log"), "a") as log:
        log.write(os.environ.get("TUMULT_LAKE_PATH", "") + "\n")
    print("Status: " + os.environ.get("FAKE_STATUS", "Completed"))
    sys.exit(int(os.environ.get("FAKE_RUN_EXIT", "0")))
if len(args) > 1 and args[1] in said:
    print(os.environ.get(said[args[1]], ""))
"""
HALTED = {"FAKE_STATUS": "Halted", "FAKE_RUN_EXIT": "1"}
# Lines of `tumult topology map`. Every map starts with the legend.
LEGEND = (
    "legend: [OK] evidenced | [BROKEN] control broken | "
    "[UNTESTED] no evidence yet | [UNKNOWN] no articles in scope"
)
EVIDENCED = LEGEND + "\n  compliance:DORA/Art.25 evidenced"
BROKEN = LEGEND + "\n[BROKEN] svc:demo-postgres (data, demo)"
# Scripts that run experiments for a store of their own.
STORE_SCRIPTS = ("demo-topology.sh", "record-demo-cast.sh", "record-demo-cast.py")


class Checkout:
    """A copy of the script in a directory laid out like the repository."""

    def __init__(self, root):
        self.root = Path(root)
        (self.root / "scripts").mkdir()
        (self.root / "demo" / "topology").mkdir(parents=True)
        shutil.copy(SCRIPT, self.root / "scripts")
        self.tumult = self.root / "tumult"
        self.tumult.write_text(FAKE_TUMULT)
        self.tumult.chmod(self.tumult.stat().st_mode | stat.S_IXUSR)

    def stores(self):
        """The store each run was given, in the order of the runs."""
        return (self.root / "runs.log").read_text().splitlines()

    def run(self, demo, **said):
        """Run one demo of the script. TUMULT_DEMO_NS skips the namespace."""
        env = {
            **os.environ,
            "TUMULT_DEMO_NS": "1",
            "TUMULT_BIN": str(self.tumult),
            "TUMULT_TOPOLOGY_DEMO_STORE": str(self.root / "store.duckdb"),
            **said,
        }
        return subprocess.run(
            ["bash", str(self.root / "scripts" / "demo-topology.sh"), demo],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )


class DemoTopologyTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.checkout = Checkout(tmp.name)

    def assert_fails(self, done, proof):
        self.assertNotEqual(done.returncode, 0, done.stdout)
        self.assertIn(f"FAIL: {proof}", done.stderr)
        self.assertNotIn(f"OK: {proof}", done.stdout)

    def test_a_map_that_shows_the_evidence_passes_proof_1(self):
        done = self.checkout.run("1", FAKE_MAP=EVIDENCED)

        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("PROOF 1 OK: Art.25 evidenced on demo-app", done.stdout)

    def test_a_run_writes_to_the_store_the_map_reads(self):
        self.checkout.run("1", FAKE_MAP=EVIDENCED)

        self.assertEqual(
            self.checkout.stores(), [str(self.checkout.root / "store.duckdb")]
        )

    def test_no_script_names_its_store_by_the_retired_variable(self):
        # `tumult run` reads TUMULT_LAKE_PATH. With the old name alone a run
        # goes to the lake of the user and the map of the demo stays empty.
        for name in STORE_SCRIPTS:
            text = SCRIPT.with_name(name).read_text()
            self.assertNotIn("TUMULT_ANALYTICS_PATH", text, name)

    def test_a_map_without_the_evidence_fails_proof_1(self):
        done = self.checkout.run("1", FAKE_MAP=LEGEND)

        self.assert_fails(done, "Art.25 evidenced on demo-app")

    def test_a_halt_a_break_and_its_cause_pass_proof_2(self):
        done = self.checkout.run(
            "2", FAKE_MAP=BROKEN, FAKE_LINEAGE="pause-database", **HALTED
        )

        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        for proof in (
            "the guard halted the run",
            "break visible on map",
            "cause attributed to pause-database",
        ):
            self.assertIn(f"PROOF 2 OK: {proof}", done.stdout)

    def test_a_run_that_does_not_halt_fails_proof_2(self):
        done = self.checkout.run("2", FAKE_MAP=BROKEN, FAKE_LINEAGE="pause-database")

        self.assert_fails(done, "the guard halted the run")

    def test_a_map_without_the_break_fails_proof_2(self):
        done = self.checkout.run(
            "2", FAKE_MAP=LEGEND, FAKE_LINEAGE="pause-database", **HALTED
        )

        self.assert_fails(done, "break visible on map")

    def test_a_lineage_without_the_cause_fails_proof_2(self):
        done = self.checkout.run("2", FAKE_MAP=BROKEN, FAKE_LINEAGE="{}", **HALTED)

        self.assert_fails(done, "cause attributed to pause-database")

    def test_a_demo_after_another_demo_still_gets_its_run(self):
        # The CLI does not write over the journal of the run before.
        self.checkout.run("1", FAKE_MAP=EVIDENCED)

        done = self.checkout.run(
            "2", FAKE_MAP=BROKEN, FAKE_LINEAGE="pause-database", **HALTED
        )

        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def test_a_demo_can_run_again(self):
        self.checkout.run("1", FAKE_MAP=EVIDENCED)

        done = self.checkout.run("1", FAKE_MAP=EVIDENCED)

        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def test_a_gate_that_enacts_nothing_fails_proof_4(self):
        done = self.checkout.run("4", FAKE_AUTOPILOT="[veto] every candidate")

        self.assert_fails(done, "gate enacted the pretrusted class")

    def test_no_proof_line_is_printed_without_its_check(self):
        # `set -e` does not see a failing `check && echo` line.
        self.assertEqual(SCRIPT.read_text().count('&& echo "PROOF'), 0)


if __name__ == "__main__":
    unittest.main()

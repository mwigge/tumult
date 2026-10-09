#!/usr/bin/env python3
"""Hermetic lifecycle checks for the daemon image smoke script."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("smoke-daemon-image.sh")
FAKE_DOCKER = r"""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
entry = {"args": args}
if "--env-file" in args:
    entry["env"] = dict(line.split("=", 1) for line in Path(args[args.index("--env-file") + 1]).read_text().splitlines())
with open(os.environ["SMOKE_CALLS"], "a") as out:
    out.write(json.dumps(entry) + "\n")
if args[0] == "inspect":
    print("true")
if args[0] == "exec" and "tumult-net-proxyd --help" in " ".join(args):
    sys.exit(1)
if args[0] == "exec" and os.environ.get("SMOKE_FAIL_PROBE") == "1":
    sys.exit(1)
if args[:2] == ["volume", "create"]:
    print(args[-1])
    if os.environ.get("SMOKE_FAIL_VOLUME") == "1":
        sys.exit(1)
"""


class SmokeImageTests(unittest.TestCase):
    def run_smoke(self, fail=False, fail_volume=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "docker"
            fake.write_text(FAKE_DOCKER)
            fake.chmod(0o700)
            calls = root / "calls.jsonl"
            env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}")
            env.update(
                SMOKE_CALLS=str(calls),
                SMOKE_FAIL_PROBE="1" if fail else "0",
                SMOKE_FAIL_VOLUME="1" if fail_volume else "0",
                TUMULT_SMOKE_TIMEOUT_SECONDS="1",
            )
            result = subprocess.run(
                ["bash", str(SCRIPT), "local/daemon:test"],
                env=env,
                capture_output=True,
                text=True,
                timeout=10,
            )
            entries = (
                [json.loads(line) for line in calls.read_text().splitlines()]
                if calls.exists()
                else []
            )
            return result, entries

    def test_fresh_container_reuses_volume_without_reprovisioning(self):
        result, calls = self.run_smoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        runs = [call for call in calls if call["args"][0] == "run"]
        self.assertEqual(len(runs), 2)
        first, second = runs
        for run in runs:
            args = run["args"]
            self.assertIn("--read-only", args)
            self.assertEqual(args[args.index("--network") + 1], "none")
            self.assertEqual(args[args.index("--user") + 1], "10001:10001")
            self.assertIn("--tmpfs", args)
            self.assertNotIn("--publish", args)
            self.assertNotIn("-p", args)
        self.assertEqual(
            first["args"][first["args"].index("--mount") + 1],
            second["args"][second["args"].index("--mount") + 1],
        )
        self.assertNotEqual(
            first["args"][first["args"].index("--name") + 1],
            second["args"][second["args"].index("--name") + 1],
        )
        self.assertIn("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD", first["env"])
        self.assertIn("KRONIKA_BOOTSTRAP_TOKEN", first["env"])
        self.assertNotIn("KRONIKA_BOOTSTRAP_ADMIN_PASSWORD", second["env"])
        self.assertNotIn("KRONIKA_BOOTSTRAP_TOKEN", second["env"])
        token = first["env"]["KRONIKA_BOOTSTRAP_TOKEN"]
        self.assertEqual(token, second["env"]["TUMULT_SMOKE_API_TOKEN"])
        self.assertNotIn(token, result.stdout + result.stderr)
        execs = "\n".join(
            " ".join(call["args"]) for call in calls if call["args"][0] == "exec"
        )
        for evidence in [
            "/readyz",
            "/healthz",
            "/api/me",
            "/api/metrics",
            "tumult-net-proxyd",
            "authenticated",
            "auth_required",
            "lake.duckdb",
        ]:
            self.assertIn(evidence, execs)
        self.assertTrue(any(call["args"][:2] == ["volume", "rm"] for call in calls))

    def test_probe_failure_is_bounded_and_cleans_up(self):
        result, calls = self.run_smoke(fail=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any(call["args"][0] == "rm" for call in calls))
        self.assertTrue(any(call["args"][:2] == ["volume", "rm"] for call in calls))
        self.assertFalse(any(call["args"][0] == "logs" for call in calls))

    def test_partial_volume_creation_is_cleaned_up(self):
        result, calls = self.run_smoke(fail_volume=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any(call["args"][:2] == ["volume", "rm"] for call in calls))

    def test_image_argument_is_required(self):
        result = subprocess.run(["bash", str(SCRIPT)], capture_output=True)
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()

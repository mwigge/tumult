#!/usr/bin/env python3
"""Hermetic checks of scripts/signoz-bulk-import.sh on a lake export folder,
with fake `docker` and `clickhouse-client` commands: no network, no
container."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("signoz-bulk-import.sh")
# One fake for both tools. It notes each call in FAKE_CALLS and answers the
# queries of the script as a SigNoz ClickHouse would: a file holds FAKE_ROWS
# rows, and the index and resource tables are there.
FAKE_TOOL = r"""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ["FAKE_CALLS"], "a") as out:
    out.write(json.dumps({"tool": Path(sys.argv[0]).name, "args": args}) + "\n")
if "--query" not in args:
    sys.exit(0)
query = args[args.index("--query") + 1]
if query.startswith("SELECT count() FROM file("):
    print(os.environ.get("FAKE_ROWS", "7"))
elif "name IN (" in query:
    print(2)
elif "system.tables" in query or query == "SELECT 1":
    print(1)
"""
SPANS = "a" * 64
OLDER = "b" * 64
NEWER = "d" * 64
LOGS = "c" * 64
CONTAINER = "signoz-clickhouse"


def snapshot(fingerprint, table="spans"):
    return f"snapshots/{table}/{fingerprint}.parquet"


def write_manifest(lake, spans):
    """The `_meta.json` of an export whose spans snapshot is `spans`."""
    meta = {
        "format_version": 2,
        "last_export_ns": 1,
        "tables": {"spans": 1, "logs": 1},
        "fingerprints": {"spans": spans, "logs": LOGS},
        "snapshots": {"spans": snapshot(spans), "logs": snapshot(LOGS, "logs")},
        "file_checksums": {},
        "excluded_tables": [],
    }
    (lake / "_meta.json").write_text(json.dumps(meta, indent=2))


class BulkImport(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        for name in ("docker", "clickhouse-client"):
            tool = bin_dir / name
            tool.write_text(FAKE_TOOL)
            tool.chmod(0o700)
        self.calls = self.root / "calls.jsonl"
        self.path = f"{bin_dir}:{os.environ['PATH']}"
        # A lake as the export leaves it: the snapshot the manifest names,
        # an older spans snapshot next to it, a logs snapshot, and a file in
        # the date layout that the export no longer writes.
        self.lake = self.root / "lake"
        for rel in (
            snapshot(SPANS),
            snapshot(OLDER),
            snapshot(LOGS, "logs"),
            "spans/date=2026-01-01/data-0.parquet",
        ):
            self.put(rel)
        write_manifest(self.lake, SPANS)

    def put(self, rel):
        path = self.lake / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"PAR1")

    def run_import(self, *args, docker=True, **env):
        """Runs the script and returns its result and the calls it made."""
        self.calls.write_text("")
        environment = {
            "PATH": self.path,
            "HOME": str(self.root),
            "FAKE_CALLS": str(self.calls),
            "KRONIKA_LAKE_DIR": str(self.lake),
        }
        if docker:
            environment["SIGNOZ_DOCKER_CONTAINER"] = CONTAINER
        environment.update(env)
        result = subprocess.run(
            ["bash", str(SCRIPT), *args],
            env=environment,
            capture_output=True,
            text=True,
            timeout=30,
        )
        calls = [json.loads(line) for line in self.calls.read_text().splitlines()]
        return result, calls

    def ledger(self):
        path = self.lake / ".signoz-import-ledger"
        if not path.exists():
            return []
        return [line.split("\t") for line in path.read_text().splitlines()]


def queries(calls):
    return [
        call["args"][call["args"].index("--query") + 1]
        for call in calls
        if "--query" in call["args"]
    ]


def inserts(calls):
    return [query for query in queries(calls) if query.startswith("INSERT INTO")]


def copies(calls):
    return [
        call["args"]
        for call in calls
        if call["tool"] == "docker" and call["args"][0] == "cp"
    ]


class Import(BulkImport):
    def test_docker_mode_imports_only_the_snapshot_the_manifest_names(self):
        result, calls = self.run_import()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        staged = f"snapshots__spans__{SPANS}.parquet"
        self.assertEqual(len(copies(calls)), 1, copies(calls))
        source, target = copies(calls)[0][1:3]
        self.assertEqual(source, str(self.lake / snapshot(SPANS)))
        self.assertTrue(target.startswith(f"{CONTAINER}:"), target)
        self.assertTrue(target.endswith(f"/{staged}"), target)
        self.assertEqual(len(inserts(calls)), 2, inserts(calls))
        for insert in inserts(calls):
            self.assertIn(f"/{staged}', Parquet)", insert)
        said = json.dumps(calls)
        for other in (OLDER, LOGS, "date="):
            self.assertNotIn(other, said)
        self.assertEqual([line[:2] for line in self.ledger()], [[snapshot(SPANS), "7"]])

    def test_a_second_run_does_not_import_the_same_snapshot_again(self):
        self.run_import()

        result, calls = self.run_import()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(inserts(calls), [])
        self.assertEqual(copies(calls), [])
        self.assertIn("already imported", result.stdout)
        self.assertEqual(len(self.ledger()), 1)

    def test_after_an_import_a_newer_snapshot_is_refused_unless_forced(self):
        self.run_import()
        self.put(snapshot(NEWER))
        write_manifest(self.lake, NEWER)

        refused, calls = self.run_import()

        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.assertEqual(inserts(calls), [])
        self.assertIn("twice", refused.stderr)
        forced, calls = self.run_import("--force")
        self.assertEqual(forced.returncode, 0, forced.stdout + forced.stderr)
        self.assertEqual(len(inserts(calls)), 2, inserts(calls))
        for insert in inserts(calls):
            self.assertIn(f"snapshots__spans__{NEWER}.parquet", insert)

    def test_local_mode_reads_the_snapshot_where_the_server_sees_the_lake(self):
        result, calls = self.run_import(docker=False, PARQUET_SERVER_DIR="/srv/lake")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual({call["tool"] for call in calls}, {"clickhouse-client"})
        self.assertEqual(len(inserts(calls)), 2, inserts(calls))
        for insert in inserts(calls):
            self.assertIn(f"file('/srv/lake/{snapshot(SPANS)}', Parquet)", insert)

    def test_a_dry_run_counts_the_snapshot_and_inserts_nothing(self):
        result, calls = self.run_import("--dry-run")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        counts = [
            query
            for query in queries(calls)
            if query.startswith("SELECT count() FROM file(")
        ]
        self.assertEqual(len(counts), 1, counts)
        self.assertIn(f"snapshots__spans__{SPANS}.parquet", counts[0])
        self.assertEqual(inserts(calls), [])
        self.assertEqual(self.ledger(), [])
        self.assertIn(snapshot(SPANS), result.stdout)


class Refusals(BulkImport):
    def test_a_lake_without_a_manifest_is_a_setup_error(self):
        (self.lake / "_meta.json").unlink()

        result, calls = self.run_import()

        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("_meta.json", result.stderr)
        self.assertEqual(inserts(calls), [])

    def test_the_old_glob_setting_is_refused(self):
        result, calls = self.run_import(PARQUET_GLOB=str(self.lake / snapshot(OLDER)))

        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("PARQUET_GLOB", result.stderr)
        self.assertEqual(inserts(calls), [])


if __name__ == "__main__":
    unittest.main()

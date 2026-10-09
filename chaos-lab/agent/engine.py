"""Bounded, evidence-driven Tumult experiments for a local teaching sandbox."""

import asyncio
import copy
import json
import logging
import math
import os
import re
import shutil
import signal
import sys
import uuid
from datetime import UTC, datetime
from pathlib import Path
from urllib.parse import urlsplit

from agent.lab_activity import (
    FAULT_DURATION_S,
    THRESHOLD_MS,
    atomic_json,
    request,
    sample,
)

LOGGER = logging.getLogger(__name__)
MAX_RUNS = 200
RUN_ID = re.compile(r"^[a-f0-9]{32}$")
CATALOGUE = [
    {
        "id": "latency",
        "title": "Slow responses",
        "description": "Add 400 ms of real response delay to the isolated teaching API.",
        "learning": "A service can return HTTP 200 while breaking a latency objective.",
        "fault": "latency",
    },
    {
        "id": "http-errors",
        "title": "HTTP failures",
        "description": "Make the teaching API return HTTP 503 during the fault window.",
        "learning": "Compare availability with latency: fast failures are still failures.",
        "fault": "http-errors",
    },
    {
        "id": "cache-outage",
        "title": "Unavailable cache",
        "description": "Simulate an unavailable cache dependency inside the teaching API.",
        "learning": "Explore dependency coupling and when graceful degradation could help.",
        "fault": "cache-outage",
    },
    {
        "id": "db-timeout",
        "title": "Database timeout",
        "description": "Simulate a database timeout: 600 ms delay followed by HTTP 503.",
        "learning": "A slow dependency consumes time before failure; discuss timeout budgets and isolation.",
        "fault": "db-timeout",
    },
]


def now() -> str:
    return datetime.now(UTC).isoformat()


class LabEngine:
    """Execute a fixed catalogue with Tumult and retain measured, private evidence."""

    def __init__(
        self, data_dir: Path, target_url: str, control_token: str, tumult_bin: str
    ) -> None:
        parsed = urlsplit(target_url)
        if (
            parsed.scheme != "http"
            or parsed.hostname not in {"target", "api", "localhost", "127.0.0.1", "::1"}
            or parsed.username
            or parsed.password
            or parsed.path not in {"", "/"}
            or parsed.query
            or parsed.fragment
        ):
            raise ValueError(
                "Target must be the isolated lab API or a loopback test server"
            )
        self.data_dir = Path(data_dir)
        self.data_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.target_url = target_url.rstrip("/")
        self.control_token = control_token
        resolved_binary = shutil.which(tumult_bin)
        self.tumult_bin = (
            str(Path(resolved_binary).resolve()) if resolved_binary else tumult_bin
        )
        self.active_run_id: str | None = None
        self._task: asyncio.Task | None = None
        self._process: asyncio.subprocess.Process | None = None
        self._records: dict[str, dict] = {}
        self._lock = asyncio.Lock()
        for path in self.data_dir.glob("*/run.json"):
            if not RUN_ID.fullmatch(path.parent.name):
                continue
            try:
                value = json.loads(path.read_text())
                if (
                    isinstance(value, dict)
                    and value.get("id") == path.parent.name
                    and isinstance(value.get("events"), list)
                    and isinstance(value.get("status"), str)
                ):
                    self._records[value["id"]] = value
            except (OSError, ValueError):
                LOGGER.warning("Unreadable run evidence: %s", path.parent.name)

    def scenarios(self) -> list[dict]:
        """Return fixed educational choices without accepting provider commands."""
        return [
            dict(
                item,
                hypothesis="All responses succeed and measured p95 stays at or below 200 ms.",
                target="Isolated teaching API; dependencies are simulated",
                duration_s=FAULT_DURATION_S,
                threshold_ms=THRESHOLD_MS,
            )
            for item in CATALOGUE
        ]

    def _folder(self, run_id: str) -> Path | None:
        return self.data_dir / run_id if RUN_ID.fullmatch(run_id) else None

    def journal_path(self, run_id: str) -> Path | None:
        folder = self._folder(run_id)
        path = folder / "journal.toon" if folder else None
        return path if path and path.is_file() else None

    def experiment_path(self, run_id: str) -> Path | None:
        folder = self._folder(run_id)
        path = folder / "experiment.toon" if folder else None
        return path if path and path.is_file() else None

    def _save(self, record: dict) -> None:
        atomic_json(self.data_dir / record["id"] / "run.json", record)

    def _event(self, record: dict, message: str) -> None:
        record["events"].append({"at": now(), "message": message})
        self._save(record)

    async def _reset(self) -> None:
        status, _ = await asyncio.to_thread(
            request, self.target_url, "/reset", self.control_token, {}
        )
        if status != 200:
            raise RuntimeError("Target reset rejected with HTTP " + str(status))

    async def initialize(self) -> None:
        """Record interrupted work and reset a previous bounded fault lease."""
        for record in self._records.values():
            if record.get("status") in {"running", "queued", "stopping", "finalizing"}:
                record.update(
                    status="interrupted",
                    phase="interrupted",
                    finished_at=now(),
                    verdict="inconclusive",
                    explanation="The lab restarted before this experiment finished. No recovery result is inferred.",
                )
                self._event(
                    record,
                    "Previous process interrupted; the fault also has a target-enforced 15-second maximum lifetime.",
                )
        try:
            await self._reset()
        except (OSError, ValueError, RuntimeError) as exc:
            LOGGER.warning("Startup target reset unavailable: %s", type(exc).__name__)

    async def health(self) -> dict:
        """Report executable and target readiness using actual checks."""
        result = {
            "tumult": False,
            "target": False,
            "active_run_id": self.active_run_id,
            "version": None,
        }
        try:
            process = await asyncio.create_subprocess_exec(
                self.tumult_bin,
                "--version",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.DEVNULL,
            )
            try:
                stdout, _ = await asyncio.wait_for(process.communicate(), 5)
            except TimeoutError:
                process.kill()
                await process.wait()
                raise
            result["tumult"] = process.returncode == 0
            result["version"] = stdout.decode(errors="replace").strip()[:100]
        except (OSError, TimeoutError):
            LOGGER.warning("Tumult executable is not ready")
        try:
            status, _ = await asyncio.to_thread(request, self.target_url, "/health")
            result["target"] = status == 200
        except (OSError, ValueError):
            LOGGER.warning("Teaching target is not ready")
        return result

    async def start(self, scenario_id: str) -> dict:
        scenario = next(
            (item for item in self.scenarios() if item["id"] == scenario_id), None
        )
        if scenario is None:
            raise ValueError("Unknown scenario")
        async with self._lock:
            if self.active_run_id:
                raise RuntimeError("An experiment is already running")
            if len(self._records) >= MAX_RUNS:
                raise RuntimeError(
                    "History limit reached (200 runs). Export evidence before resetting lab storage."
                )
            run_id = uuid.uuid4().hex
            (self.data_dir / run_id).mkdir(mode=0o700)
            record = {
                "id": run_id,
                "scenario_id": scenario_id,
                "title": scenario["title"],
                "status": "running",
                "phase": "preparing",
                "started_at": now(),
                "finished_at": None,
                "baseline": None,
                "during": None,
                "recovery": None,
                "events": [],
                "verdict": "pending",
                "explanation": "Collecting real observations.",
                "journal_available": False,
                "native_status": None,
            }
            self._event(
                record,
                "Starting a fixed Tumult experiment against the isolated teaching API.",
            )
            self._records[run_id] = record
            self.active_run_id = run_id
            self._task = asyncio.create_task(self._execute(record, scenario))
            await asyncio.sleep(0)
            return copy.deepcopy(record)

    async def get(self, run_id: str) -> dict | None:
        record = self._records.get(run_id)
        if not record:
            return None
        self._refresh(record)
        return copy.deepcopy(record)

    async def list_runs(self) -> list[dict]:
        return [
            copy.deepcopy(self._records[run_id])
            for run_id in sorted(
                self._records,
                key=lambda key: self._records[key].get("started_at", ""),
                reverse=True,
            )
        ]

    def _refresh(self, record: dict) -> None:
        folder = self.data_dir / record["id"]
        for stage in ("baseline", "during", "recovery"):
            path = folder / (stage + ".json")
            if path.exists():
                try:
                    metrics = json.loads(path.read_text())
                    if (
                        not isinstance(metrics, dict)
                        or not isinstance(metrics.get("healthy"), bool)
                        or any(
                            not isinstance(metrics.get(key), (int, float))
                            or not math.isfinite(metrics[key])
                            or metrics[key] < 0
                            for key in (
                                "requests",
                                "errors",
                                "error_rate",
                                "p95_ms",
                                "mean_ms",
                            )
                        )
                    ):
                        raise ValueError("Invalid metrics evidence")
                    record[stage] = metrics
                except (OSError, ValueError):
                    record[stage] = None
                    record.update(
                        status="failed",
                        phase="failed",
                        verdict="inconclusive",
                        explanation="Saved measurement evidence is unreadable or invalid; no result is inferred.",
                    )
                    LOGGER.warning(
                        "Invalid %s evidence for run %s", stage, record["id"]
                    )
        if record["status"] == "running" and (folder / "phase.json").exists():
            try:
                phase = json.loads((folder / "phase.json").read_text()).get("phase")
                if phase not in {"baseline", "during", "recovery", "inject", "reset"}:
                    raise ValueError("Invalid phase evidence")
                record["phase"] = phase
            except (OSError, ValueError, AttributeError):
                LOGGER.warning("Unreadable phase for run %s", record["id"])
        record["journal_available"] = self.journal_path(record["id"]) is not None

    def _experiment(self) -> str:
        helper = str(Path(__file__).with_name("lab_activity.py").resolve())

        def activity(name: str, kind: str = "action", tolerance: bool = False) -> str:
            lines = [
                f"  - name: {name}",
                f"    activity_type: {kind}",
                "    provider:",
                "      type: process",
                "      path: " + json.dumps(sys.executable),
                "      arguments[2]: " + json.dumps(helper) + ", " + json.dumps(name),
                "      timeout_s: 20.0",
            ]
            if tolerance:
                lines.extend(
                    [
                        "    tolerance:",
                        "      type: regex",
                        "      pattern: " + json.dumps(r'"healthy":\s*true'),
                    ]
                )
            return "\n".join(lines)

        return "\n".join(
            [
                "title: Tumult Chaos Lab bounded experiment",
                "description: Fixed process activities against an isolated educational HTTP target",
                "tags[2]: lab, simulation",
                "steady_state_hypothesis:",
                "  title: All responses succeed and p95 is at most 200 ms",
                "  probes[1]:",
                "\n".join(
                    "  " + line
                    for line in activity("steady", "probe", True).splitlines()
                ),
                "method[2]:",
                activity("inject"),
                activity("during", "probe", True),
                "rollbacks[2]:",
                activity("reset"),
                activity("recovery", "probe", True),
                "",
            ]
        )

    async def _terminate(self) -> None:
        process = self._process
        if process is None or process.returncode is not None:
            return
        try:
            os.killpg(process.pid, signal.SIGINT)
        except ProcessLookupError:
            return
        try:
            await asyncio.wait_for(process.wait(), 3)
        except TimeoutError:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                LOGGER.debug("Experiment exited before escalation")
            await process.wait()

    async def _execute(self, record: dict, scenario: dict) -> None:
        folder = self.data_dir / record["id"]
        outcome = "failed"
        try:
            await self._reset()
            experiment = folder / "experiment.toon"
            experiment.write_text(self._experiment())
            experiment.chmod(0o600)
            environment = {
                "PATH": os.defpath,
                "HOME": str(folder),
                "LAB_TARGET_URL": self.target_url,
                "LAB_CONTROL_TOKEN": self.control_token,
                "LAB_RUN_DIR": str(folder.resolve()),
                "LAB_FAULT": scenario["fault"],
                "NO_COLOR": "1",
            }
            with (folder / "tumult.log").open("wb") as log:
                os.chmod(folder / "tumult.log", 0o600)
                launch = asyncio.create_task(
                    asyncio.create_subprocess_exec(
                        self.tumult_bin,
                        "run",
                        str(experiment.resolve()),
                        "--journal-path",
                        str((folder / "journal.toon").resolve()),
                        "--rollback-strategy",
                        "always",
                        "--no-ingest",
                        stdout=log,
                        stderr=log,
                        cwd=folder,
                        env=environment,
                        start_new_session=True,
                    )
                )
                try:
                    self._process = await asyncio.shield(launch)
                except asyncio.CancelledError:
                    self._process = await launch
                    raise
                await asyncio.wait_for(self._process.wait(), 45)
            self._refresh(record)
            if (
                not record["baseline"]
                or not record["during"]
                or not record["recovery"]
                or not record["journal_available"]
            ):
                raise RuntimeError(
                    "Tumult did not produce a complete experiment; inspect the saved journal when available"
                )
            journal = self.journal_path(record["id"])
            native_match = (
                re.search(r"^status: ([a-z_]+)$", journal.read_text(), re.MULTILINE)
                if journal
                else None
            )
            record["native_status"] = native_match.group(1) if native_match else None
            if record["native_status"] not in {
                "completed",
                "deviated",
            } or self._process.returncode not in {0, 1}:
                raise RuntimeError(
                    "Tumult reported an unsuccessful native experiment outcome"
                )
            outcome = "completed"
            record.update(status="finalizing", phase="cleanup")
            if not record["baseline"]["healthy"] or not record["recovery"]["healthy"]:
                record.update(
                    verdict="inconclusive",
                    explanation="The baseline or recovery failed the objective. Resolve that before interpreting the fault window.",
                )
            elif not record["during"]["healthy"]:
                record.update(
                    verdict="deviated",
                    explanation="The measured fault window broke the objective, and the final recovery probe passed. Native status reports Tumult's later steady-state observation; it does not erase the recorded fault-window failures.",
                )
            else:
                record.update(
                    verdict="held",
                    explanation="The objective held for these eight samples. This small teaching experiment does not establish production resilience.",
                )
            self._event(
                record,
                "Tumult finished its fault method and rollback; baseline, fault-window and recovery evidence are available.",
            )
        except asyncio.CancelledError:
            outcome = "stopped"
            record.update(
                status="stopping",
                phase="cleanup",
                verdict="inconclusive",
                explanation="Stopped by the learner. Partial measurements are preserved; recovery is checked after reset.",
            )
            await self._terminate()
            self._event(
                record, "Stop requested; the Tumult process group was terminated."
            )
        except (OSError, ValueError, RuntimeError, TimeoutError) as exc:
            await self._terminate()
            outcome = "failed"
            record.update(
                status="finalizing",
                phase="cleanup",
                verdict="inconclusive",
                explanation=str(exc)[:300],
            )
            self._event(record, "Experiment could not finish: " + type(exc).__name__)
        finally:
            try:
                await self._reset()
                if not (folder / "recovery.json").exists():
                    recovery = await asyncio.to_thread(sample, self.target_url)
                    atomic_json(folder / "recovery.json", recovery)
                    self._event(
                        record,
                        "Recovery sampled by the lab safety cleanup after the Tumult process ended.",
                    )
                self._event(record, "Target reset acknowledged.")
            except (OSError, ValueError, RuntimeError) as exc:
                outcome = "failed"
                record.update(
                    verdict="inconclusive",
                    explanation="Cleanup could not contact the target. The target fault expires automatically within 15 seconds.",
                )
                self._event(record, "Reset failed: " + type(exc).__name__)
            try:
                self._refresh(record)
                if record["status"] == "failed":
                    outcome = "failed"
                record.update(
                    status=outcome,
                    phase="complete" if outcome == "completed" else outcome,
                )
                record["finished_at"] = now()
                self._save(record)
            finally:
                self.active_run_id = None
                self._process = None

    async def stop(self, run_id: str) -> dict | None:
        if run_id not in self._records:
            return None
        if run_id == self.active_run_id and self._task:
            if (
                self._records[run_id]["status"] == "running"
                and not self._task.cancelling()
            ):
                self._task.cancel()
            await asyncio.shield(self._task)
        return await self.get(run_id)

    async def close(self) -> None:
        if self.active_run_id:
            await self.stop(self.active_run_id)

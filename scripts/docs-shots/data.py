#!/usr/bin/env python3
"""Add the data the docs screenshots show to the seeded docs daemon.

shoot.sh runs this after the demo seed. It reads the stack of this run from
<work>/stack.json and writes the ids the page list needs to <work>/ids.json.

The steps, in order:
  1. performance  four builds of a loopback checkout service, three soak
                  runs of each with the tumult CLI on this host, and a stored
                  verdict after each new build
  2. queue        two message-queue experiments, so the Star Map focus on
                  the queue has neighbours
  3. risk         one accepted risk on the Resilience page
  4. governed     a definition bound to env dev, a run that waits for
                  approval, and a run that a second person approved and
                  that runs until the pictures stop it
  5. schedules, webhooks, a third user and two generated reports
  6. the HTML report of the last soak run, for the report picture
  7. dr           DR exercises run by hand at earlier dates, signed off by
                  a second person, and a planned backup, for the Disaster
                  recovery page

The CLI runs with a HOME and a lake of its own inside the work dir, so it
never reads or writes the lake of the person who runs it.

Usage: data.py <work dir>
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
PROOF = ROOT / "scripts/proof/rip-phase4"
EXPERIMENTS = HERE / "experiments"
DEMO_EXPERIMENTS = ROOT / "demo/kronika/experiments"
GOVERNED = EXPERIMENTS / "mq-consumer-pause.toon"
# The checkout builds: version and the arguments of the loopback service
# (delay in ms, then optionally every n-th request slow by m ms).
BUILDS = (
    ("1.0.0", ("2",)),
    ("1.1.0", ("40",)),
    ("1.2.0", ("2",)),
    ("1.3.0", ("3", "6", "60")),
)
SOAKS_PER_BUILD = 3
# How long the stopped run holds its fault before it would end by itself.
# The pictures stop it long before.
STOP_HOLD_S = "900"
DAY_NS = 86_400 * 10**9
# The DR exercises of the demo targets that people ran by hand: name, type,
# target, days ago, recovery time, RTO, RPO and the data lost, in seconds.
DR_EXERCISES = (
    ("storage restore from backup", "restore", "storage", 350, 1800, 3600, 900, 300),
    ("config-service failover", "failover", "config-service", 300, 540, 900, 60, 20),
    ("config-service failover", "failover", "config-service", 120, 420, 900, 60, 10),
    ("batch-worker drill", "drill", "batch-worker", 400, 1200, 900, None, None),
)


def log(message: str) -> None:
    sys.stdout.write(f"   data: {message}\n")
    sys.stdout.flush()


class Api:
    """The JSON API of the docs daemon, as one bearer token."""

    def __init__(self, url: str, token: str):
        self.url = url
        self.token = token

    def call(self, method: str, path: str, body: dict | None = None) -> dict:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            self.url + path,
            data=data,
            method=method,
            headers={
                "Authorization": f"Bearer {self.token}",
                "content-type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                text = response.read().decode()
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace")[:400]
            raise SystemExit(
                f"{method} {path} -> HTTP {error.code}: {detail}"
            ) from None
        return json.loads(text) if text.strip() else {}

    def get(self, path: str) -> dict:
        return self.call("GET", path)

    def post(self, path: str, body: dict | None = None) -> dict:
        return self.call("POST", path, body if body is not None else {})


class Stack:
    """The stack of this run, as shoot.sh wrote it."""

    def __init__(self, work: Path):
        self.work = work
        stack = json.loads((work / "stack.json").read_text(encoding="utf-8"))
        self.url = stack["url"]
        self.grpc = stack["grpc"]
        self.container = stack["container"]
        self.target_port = stack["target_port"]
        self.token = stack["token"]
        self.ingest_token = stack["ingest_token"]
        self.tumult = stack["tumult"]
        self.admin = Api(self.url, self.token)
        self.home = work / "home"
        self.home.mkdir(exist_ok=True)

    def cli_env(self) -> dict:
        """The environment of the tumult CLI: a private HOME and lake, and the
        docs daemon for the journal import and the telemetry."""
        env = dict(os.environ)
        env.update(
            # The run logs the user that started it; the pictures show no
            # name of the machine they were taken on.
            USER="docs",
            LOGNAME="docs",
            HOME=str(self.home),
            TUMULT_LAKE_PATH=str(self.home / "lake.duckdb"),
            TUMULT_DAEMON_URL=self.url,
            TUMULT_DAEMON_TOKEN=self.token,
            TUMULT_OTEL_ENABLED="true",
            OTEL_EXPORTER_OTLP_ENDPOINT=self.grpc,
            OTEL_EXPORTER_OTLP_HEADERS=f"authorization=Bearer {self.ingest_token}",
            OTEL_SERVICE_NAME="tumult",
        )
        return env

    def run(self, toon: Path, journal: Path, *args: str) -> int:
        """One `tumult run`. A deviated run is data too, so a non-zero exit
        is logged and returned, not raised."""
        out = journal.with_suffix(".out")
        command = [self.tumult, "run", str(toon), "--journal-path", str(journal)]
        command += ["--force", *args]
        with out.open("w", encoding="utf-8") as handle:
            code = subprocess.run(
                command,
                env=self.cli_env(),
                stdout=handle,
                stderr=subprocess.STDOUT,
                check=False,
            ).returncode
        log(f"run {toon.stem} -> exit {code}")
        return code


def wait_ready(port: int) -> None:
    for _ in range(100):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/ready", timeout=1):
                return
        except OSError:
            time.sleep(0.1)
    raise SystemExit(f"the checkout service on port {port} did not start")


def emit_deploy(stack: Stack, version: str) -> None:
    """One server span of the new build, the way its own SDK would send it."""
    span = subprocess.run(
        [sys.executable, str(PROOF / "otlp_span.py"), "checkout", version, "demo"],
        capture_output=True,
        check=True,
    ).stdout
    request = urllib.request.Request(
        stack.url + "/v1/traces",
        data=span,
        method="POST",
        headers={
            "content-type": "application/x-protobuf",
            "Authorization": f"Bearer {stack.ingest_token}",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        log(f"deploy span of checkout {version} -> HTTP {response.status}")


def performance(stack: Stack) -> Path:
    """Soak runs of four checkout builds and a stored verdict per new build.
    Returns the journal of the last soak run."""
    soak = EXPERIMENTS / "checkout-soak.toon"
    runs = stack.work / "perf"
    runs.mkdir(exist_ok=True)
    port = stack.target_port
    journal = runs / "none"
    for number, (version, args) in enumerate(BUILDS):
        with (runs / f"service-{version}.log").open("w") as service_log:
            service = subprocess.Popen(
                [sys.executable, str(PROOF / "target.py"), str(port), *args],
                stdout=service_log,
                stderr=subprocess.STDOUT,
            )
        try:
            wait_ready(port)
            emit_deploy(stack, version)
            time.sleep(5)
            for soak_number in range(1, SOAKS_PER_BUILD + 1):
                journal = runs / f"checkout-{version}-{soak_number}.journal.toon"
                stack.run(soak, journal, "--var", f"port={port}")
            time.sleep(5)
        finally:
            service.terminate()
            service.wait(timeout=10)
        if number == 0:
            continue
        verdict = subprocess.run(
            [stack.tumult, "perf", "checkout", "--env", "demo", "--store"]
            + ["--fail-on", "never"],
            env=stack.cli_env(),
            capture_output=True,
            text=True,
            check=False,
        )
        log(f"perf verdict after checkout {version} -> exit {verdict.returncode}")
        if verdict.returncode != 0:
            raise SystemExit(verdict.stdout + verdict.stderr)
    return journal


def queue(stack: Stack) -> None:
    for name in ("mq-broker-restart", "mq-backlog-drain"):
        journal = stack.work / f"{name}.journal.toon"
        stack.run(EXPERIMENTS / f"{name}.toon", journal)


def accept_risk(stack: Stack) -> None:
    risks = stack.admin.get("/api/risks")["risks"]
    found = [
        risk["id"]
        for risk in risks
        if risk["rule"] == "no_readiness_probe" and "config-service" in risk["subject"]
    ]
    if not found:
        raise SystemExit("no readiness-probe risk of config-service to accept")
    stack.admin.post(
        f"/api/risks/{found[0]}/suppress",
        {
            "reason": "Accepted until the config-service chart adds a readiness"
            " probe (planned next sprint)",
            "expires_in_days": 14,
        },
    )
    log("accepted one risk of config-service")


def bind(stack: Stack, registry_id: str, holds: list[str]) -> None:
    """Bind the definition to env dev for each value of hold_s, the way an
    administrator reviews an execution (see seed-gameday.sh of the demo)."""
    path = "/data/lake.execution-bindings.json"
    current = subprocess.run(
        ["docker", "exec", stack.container, "cat", path],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    bindings = json.loads(current)
    for hold in holds:
        dry = stack.admin.post(
            "/api/runs/dry-run",
            {"registry_id": registry_id, "env": "dev", "vars": {"hold_s": hold}},
        )
        bindings.append({"sha256": dry["binding_hash"], "env": "dev", "target": None})
    subprocess.run(
        ["docker", "exec", "-i", stack.container, "sh", "-c"]
        + [f"cat > {path}.tmp && mv {path}.tmp {path}"],
        input=json.dumps(bindings),
        text=True,
        check=True,
    )
    log(f"bound {registry_id} to env dev for hold_s {', '.join(holds)}")


def user_id(stack: Stack, name: str) -> str:
    users = stack.admin.get("/api/users")["users"]
    return next(user["id"] for user in users if user["username"] == name)


def approver(stack: Stack) -> Api:
    """The API as bob, the approver the demo seed creates."""
    minted = stack.admin.post(
        "/api/tokens", {"name": "bob-approvals", "user_id": user_id(stack, "bob")}
    )
    return Api(stack.url, minted["token"])


def request_run(stack: Stack, registry_id: str, hold: str) -> str:
    run = stack.admin.post(
        "/api/runs",
        {"registry_id": registry_id, "env": "dev", "vars": {"hold_s": hold}},
    )
    return run["run_id"]


def wait_state(stack: Stack, run_id: str, wanted: str) -> None:
    state = ""
    for _ in range(120):
        state = stack.admin.get(f"/api/runs/{run_id}")["run"]["state"]
        if state == wanted:
            return
        time.sleep(1)
    raise SystemExit(f"run {run_id} is {state}, not {wanted}")


def governed(stack: Stack) -> dict:
    toon = GOVERNED.read_text(encoding="utf-8")
    # The definition keeps its ${hold_s}; validation needs one value for it.
    registered = stack.admin.post(
        "/api/runs/validate", {"toon": toon, "vars": {"hold_s": "30"}}
    )
    if not registered.get("valid"):
        raise SystemExit(f"the governed definition is not valid: {registered}")
    registry_id = registered["registry_id"]
    bind(stack, registry_id, ["30", "120", STOP_HOLD_S])
    pending = request_run(stack, registry_id, "120")
    log(f"run {pending} waits for approval")
    return {"registry.governed": registry_id, "run.pending": pending}


def running(stack: Stack, registry_id: str) -> str:
    """A run that bob approved and that holds its fault until it is stopped."""
    run_id = request_run(stack, registry_id, STOP_HOLD_S)
    approver(stack).post(
        f"/api/runs/{run_id}/approve",
        {"note": "reviewed the pause window with the team"},
    )
    wait_state(stack, run_id, "running")
    log(f"run {run_id} is running")
    return run_id


def schedules(stack: Stack, registry_id: str) -> None:
    gameday = stack.admin.get("/api/registry")["definitions"]
    worker = next(e["id"] for e in gameday if e["name"].startswith("worker-process"))
    for name, registry, interval, hold in (
        ("nightly consumer pause", registry_id, 86400, "30"),
        ("weekly worker kill", worker, 604800, None),
    ):
        body = {
            "name": name,
            "registry_id": registry,
            "interval_s": interval,
            "env": "dev",
        }
        if hold:
            body["vars"] = {"hold_s": hold}
        stack.admin.post("/api/schedules", body)
    log("created two schedules")


def webhooks(stack: Stack) -> None:
    # Hosts under the reserved .example name. One sink is off and the other
    # listens only for events this session never makes, so nothing is sent.
    made = stack.admin.post(
        "/api/webhooks",
        {"name": "chat-ops", "url": "https://chat.example/hooks/tumult"},
    )
    stack.admin.post(
        "/api/webhooks",
        {
            "name": "incident-tool",
            "url": "https://incidents.example/api/tumult",
            "events": ["rejected", "break_glass"],
        },
    )
    stack.admin.post(f"/api/webhooks/{made['id']}/enable", {"enabled": False})
    log("created two webhooks, one of them off")


def third_user(stack: Stack) -> None:
    password = os.urandom(18).hex()
    stack.admin.post(
        "/api/users", {"username": "erin", "password": password, "role": "operator"}
    )
    log("created erin, an operator")


def reports(stack: Stack) -> None:
    stack.admin.post(
        "/api/reports/v2/generate", {"type": "executive-digest", "period": "7d"}
    )
    stack.admin.post(
        "/api/reports/v2/generate",
        {"type": "evidence-pack", "period": "30d", "framework": "dora"},
    )
    log("generated an executive digest and a DORA evidence pack")


def html_report(stack: Stack, journal: Path) -> None:
    out = stack.work / "report.html"
    subprocess.run(
        [stack.tumult, "report", str(journal), "--output", str(out)],
        env=stack.cli_env(),
        check=True,
        capture_output=True,
    )
    log(f"wrote the HTML report of {journal.name}")


def dr(stack: Stack) -> None:
    """DR exercises that admin enters and bob signs off, at earlier dates,
    and a planned backup that no test has covered yet."""
    bob = approver(stack)
    now = time.time_ns()
    for name, kind, target, days, recovery, rto, rpo, lost in DR_EXERCISES:
        made = stack.admin.post(
            "/api/manual/experiments",
            {
                "experiment_name": name,
                "exercise_type": kind,
                "executed_at_ns": now - days * DAY_NS,
                "hypothesis": f"{target} is back within its RTO",
                "method": "Follow the DR runbook against the demo stack",
                "outcome_status": "passed" if recovery <= rto else "failed",
                "hypothesis_met": recovery <= rto,
                "target_system": target,
                "target_environment": "demo",
                "recovery_time_s": recovery,
                "rto_target_s": rto,
                "rpo_target_s": rpo,
                "rpo_measured_s": lost,
                "data_verified": True if lost is not None else None,
                "attestation": "I attest this record reflects the exercise as run.",
            },
        )
        stack.admin.post(f"/api/manual/experiments/{made['id']}/submit")
        bob.post(
            f"/api/manual/experiments/{made['id']}/verify",
            {"note": "checked the runbook log"},
        )
    stack.admin.post(
        "/api/manual/plans",
        {
            "name": "checkout database backup",
            "exercise_type": "backup",
            "target_system": "checkout",
            "target_environment": "demo",
            "owner": "admin",
            "due_at_ns": now + 20 * DAY_NS,
            "rto_target_s": 600,
            "rpo_target_s": 300,
        },
    )
    log(f"entered {len(DR_EXERCISES)} DR exercises and planned a backup")


def newest_experiment(stack: Stack, prefix: str) -> str:
    rows = stack.admin.get("/api/experiments?range=24h&limit=500")["experiments"]
    found = [row for row in rows if (row.get("name") or "").startswith(prefix)]
    if not found:
        raise SystemExit(f"no experiment named {prefix}")
    return max(found, key=lambda row: row["started_ns"])["id"]


def ids(stack: Stack) -> dict:
    gamedays = stack.admin.get("/api/gamedays")["gamedays"]
    return {
        "experiment.cpu": newest_experiment(stack, "host cpu burn"),
        "experiment.soak": newest_experiment(stack, "checkout soak"),
        "gameday": gamedays[0]["id"],
    }


def main() -> int:
    if len(sys.argv) != 2:
        sys.stderr.write(__doc__)
        return 2
    stack = Stack(Path(sys.argv[1]))
    journal = performance(stack)
    queue(stack)
    accept_risk(stack)
    found = governed(stack)
    schedules(stack, found["registry.governed"])
    webhooks(stack)
    third_user(stack)
    reports(stack)
    html_report(stack, journal)
    dr(stack)
    found.update(ids(stack))
    found["run.stop"] = running(stack, found["registry.governed"])
    (stack.work / "ids.json").write_text(json.dumps(found, indent=1), encoding="utf-8")
    log(f"ids: {json.dumps(found)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

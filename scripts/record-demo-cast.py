#!/usr/bin/env python3
"""Generate docs/demo/tumult-demo.cast from a REAL demo run.

Runs the 2.14-2.16 showcase (topology, compliance lineage, break
attribution, recommendation, autopilot gate) against the live demo stack
and renders an asciinema v2 cast with typed-command pacing. Every output
byte shown comes from the actual commands — nothing is scripted output.

Run via scripts/demo-topology.sh's namespace wrapper (container hostnames
must resolve) with the demo stack up:
    TUMULT_DEMO_NS=... see record-demo-cast.sh
"""
import json
import os
import re
import subprocess
import sys

WIDTH, HEIGHT = 110, 35
BIN = os.environ.get("TUMULT_BIN", "./target/debug/tumult")
STORE = os.environ["TUMULT_ANALYTICS_PATH"]

COPPER = "\x1b[38;5;173m"
DIM = "\x1b[2m"
BOLD = "\x1b[1m"
GREEN = "\x1b[1;32m"
RESET = "\x1b[0m"

events = []
clock = [0.6]


def emit(text, dt=0.0):
    clock[0] += dt
    events.append([round(clock[0], 3), "o", text])


def type_command(cmd):
    emit(f"{GREEN}❯{RESET} ", 0.4)
    for ch in cmd:
        emit(ch, 0.018 if ch != " " else 0.03)
    emit("\r\n", 0.35)


def run(cmd_display, argv, tail=None, grep=None, pause_after=1.4, env=None):
    """Type the command, run it for real, stream its (possibly filtered) output."""
    type_command(cmd_display)
    run_env = dict(os.environ, **(env or {}))
    proc = subprocess.run(argv, capture_output=True, text=True, env=run_env)
    out = (proc.stdout or "") + (proc.stderr or "")
    lines = out.rstrip("\n").splitlines()
    if grep:
        lines = [l for l in lines if re.search(grep, l)]
    if tail:
        lines = lines[-tail:]
    for line in lines:
        # Word-aware wrap at cast width so the player never scrolls oddly
        # and words stay whole.
        while len(line) > WIDTH - 2:
            cut = line.rfind(" ", 0, WIDTH - 2)
            if cut < 40:  # no sane break point — hard cut
                cut = WIDTH - 2
            emit(line[:cut] + "\r\n", 0.02)
            line = "    " + line[cut:].lstrip()
        emit(line + "\r\n", 0.028)
    emit("", pause_after)
    return out


def section(title):
    emit("\r\n", 0.8)
    emit(f"{COPPER}{BOLD}── {title} {RESET}{DIM}{'─' * max(0, WIDTH - len(title) - 6)}{RESET}\r\n", 0.05)
    emit("", 0.7)


def comment(text):
    emit(f"{DIM}# {text}{RESET}\r\n", 0.5)


T = BIN
section("tumult 2.16 · topology · compliance lineage · autopilot")
run(f"tumult --version", [T, "--version"], pause_after=0.8)

section("1 · declare the service topology (reviewed TOML, never guessed)")
run("tumult topology import demo/topology/topology.toml",
    [T, "topology", "import", "demo/topology/topology.toml", "--store", STORE])

section("2 · run chaos with compliance mappings")
comment("a latency drill on demo-app, mapped to DORA Art. 25 — completes, produces evidence")
run("tumult run demo/experiments/demo-net.toon --force",
    [T, "run", "demo/experiments/demo-net.toon", "--force"], grep=r"Running|Status|Method|Ingested")
comment("pause the database behind a safety guard watching demo-app — the guard halts the run")
run("tumult run demo/experiments/demo-guard-halt.toon --force",
    [T, "run", "demo/experiments/demo-guard-halt.toon", "--force"], grep=r"Running|Status|Rollbacks|Ingested|halt")

section("3 · where does compliance break? (and WHY)")
run("tumult topology map --framework DORA",
    [T, "topology", "map", "--framework", "DORA", "--store", STORE], pause_after=2.2)

section("4 · weight decisions by real traffic (OTel span rates, straight from SigNoz)")
run("docker exec demo-signoz clickhouse client -q 'SELECT serviceName, count() FROM ...'",
    ["docker", "exec", "demo-signoz", "clickhouse", "client", "-q",
     "SELECT serviceName, count() FROM signoz_traces.distributed_signoz_index_v3 "
     "WHERE timestamp > now() - INTERVAL 60 MINUTE GROUP BY serviceName"])
crit = subprocess.run(
    ["docker", "exec", "demo-signoz", "clickhouse", "client", "-q",
     "SELECT serviceName, count() FROM signoz_traces.distributed_signoz_index_v3 "
     "WHERE timestamp > now() - INTERVAL 60 MINUTE GROUP BY serviceName FORMAT JSON"],
    capture_output=True, text=True).stdout
crit_path = "/tmp/tumult-cast-criticality.json"
try:
    data = json.loads(crit)["data"]
    with open(crit_path, "w") as f:
        json.dump({r["serviceName"]: float(r["count()"]) for r in data}, f)
except (json.JSONDecodeError, KeyError):
    with open(crit_path, "w") as f:
        f.write("{}")
run("TUMULT_CRITICALITY_FILE=rates.json tumult topology recommend --limit 3",
    [T, "topology", "recommend", "--limit", "3", "--store", STORE],
    env={"TUMULT_CRITICALITY_FILE": crit_path}, grep=r"svc:|observed traffic|score|reason|—|-", tail=12)

section("5 · the autopilot decides — and shows its work")
comment("deterministic recommender + 14-rule gate; decisions persisted BEFORE anything runs")
run("tumult autopilot once --policy demo/topology/autopilot.toml --execute --limit 4",
    [T, "autopilot", "once", "--policy", "demo/topology/autopilot.toml",
     "--execute", "--limit", "4", "--store", STORE], pause_after=2.0)
comment("same pass minutes later: the cooldown rule downgrades the repeat to the human queue")
out = run("tumult autopilot once --policy demo/topology/autopilot.toml --execute --limit 1",
          [T, "autopilot", "once", "--policy", "demo/topology/autopilot.toml",
           "--execute", "--limit", "1", "--store", STORE])

comment("structural consent: this policy variant does not enroll demo-postgres — injection is impossible")
run("tumult autopilot once --policy demo/topology/autopilot-unenrolled.toml --limit 1",
    [T, "autopilot", "once", "--policy", "demo/topology/autopilot-unenrolled.toml",
     "--limit", "1", "--store", STORE])

section("6 · humans stay in the loop — vetoes are feedback")
status = subprocess.run(
    [T, "autopilot", "status", "--store", STORE, "--format", "json"],
    capture_output=True, text=True).stdout
deny_id = ""
try:
    rows = json.loads(status)
    rows = rows.get("decisions", rows)
    for r in rows:
        if r["verdict"] in ("propose", "downgrade") and not r.get("last_event"):
            deny_id = r["id"]
            break
except json.JSONDecodeError:
    pass
if deny_id:
    run(f'tumult autopilot deny {deny_id[:8]}… --reason "not this quarter"',
        [T, "autopilot", "deny", deny_id, "--reason", "not this quarter", "--store", STORE])
run("tumult autopilot status", [T, "autopilot", "status", "--store", STORE], tail=8)

section("7 · every decision is a graph citizen + an immutable archive")
run("tumult chaosgraph query --kind recommendation",
    [T, "chaosgraph", "query", "--kind", "recommendation", "--store", STORE], tail=6)
run("tumult autopilot export ./archive",
    [T, "autopilot", "export", "/tmp/tumult-cast-archive", "--store", STORE])

emit("\r\n", 1.0)
emit(f"{COPPER}{BOLD}every verdict reproducible from (policy hash, inputs) — tumult.rs{RESET}\r\n", 0.1)
emit("", 2.5)

header = {"version": 2, "width": WIDTH, "height": HEIGHT,
          "timestamp": 1783800000,
          "env": {"SHELL": "/bin/zsh", "TERM": "xterm-256color"}}
with open(sys.argv[1], "w") as f:
    f.write(json.dumps(header) + "\n")
    for ev in events:
        f.write(json.dumps(ev) + "\n")
print(f"cast written: {sys.argv[1]} ({len(events)} events, {clock[0]:.0f}s)")

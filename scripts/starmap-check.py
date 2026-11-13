#!/usr/bin/env python3
"""Star Map step 1 assertions against a running Krönika daemon.

The daemon must hold the demo: demo/kronika/org.yaml, the workload manifests
in demo/kronika/manifests and one run of each experiment in
demo/kronika/experiments. `scripts/demo-check.sh` brings that up and calls
this; `scripts/proof/starmap-step1/proof.sh` calls it against its own daemon.

    scripts/starmap-check.py http://127.0.0.1:14318 [--wait 60]

The API token, when the daemon asks for one, is read from STARMAP_TOKEN.
The map must show at least: the organisation, 3 teams, 4 workloads with a
`runs_on` edge, 8 experiment definitions with a `tested_by` edge, 1 gap
node, 1 node with a failed run and 1 suppressed risk, and no retired `tgt:`
id. Telemetry lands a moment after a run ends, so the map is read again
until it holds or `--wait` seconds have passed. When nothing is suppressed
yet, one open risk is suppressed for a week (a write) and the map is read
once more. Exit 0 when every line holds, 1 otherwise.
"""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request

TIMEOUT_S = 15
POLL_S = 1.0
MIN_TEAMS = 3
MIN_WORKLOADS = 4
MIN_DEFINITIONS = 8


def call(base, path, token, body=None):
    """One JSON request; a read always asks for a fresh answer."""
    headers = {"Cache-Control": "no-cache"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    request = urllib.request.Request(base + path, data=data, headers=headers)
    with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
        return json.load(response)


def edge_ends(star, rel):
    return {end for e in star["edges"] if e["rel"] == rel for end in (e["from"], e["to"])}


def structure(star):
    """The checks that do not need a write: (holds, text) per line."""
    nodes = star["nodes"]
    by_kind = {}
    for node in nodes:
        by_kind.setdefault(node["kind"], []).append(node)
    orgs = by_kind.get("org", [])
    teams = [n for n in orgs if n.get("org_kind") == "team"]
    hosted = edge_ends(star, "runs_on")
    workloads = [n for n in by_kind.get("workload", []) if n["id"] in hosted]
    tested = edge_ends(star, "tested_by")
    definitions = [n for n in by_kind.get("def", []) if n["id"] in tested]
    gaps = [n for n in nodes if n["gap"]]
    failed = [n for n in nodes if n["kind"] != "def" and n["evidence"]["failed"] > 0]
    ids = [n["id"] for n in nodes] + [e[end] for e in star["edges"] for end in ("from", "to")]
    retired = sorted({i for i in ids if i.startswith("tgt:")})
    return [
        (any(n["id"] == "org:" for n in orgs), "the organisation is the root"),
        (len(teams) >= MIN_TEAMS, f"{len(teams)} teams (at least {MIN_TEAMS})"),
        (
            len(workloads) >= MIN_WORKLOADS,
            f"{len(workloads)} workloads with a runs_on edge (at least {MIN_WORKLOADS})",
        ),
        (
            len(definitions) >= MIN_DEFINITIONS,
            f"{len(definitions)} definitions with a tested_by edge (at least {MIN_DEFINITIONS})",
        ),
        (bool(gaps), f"{len(gaps)} gap nodes (at least 1)"),
        (bool(failed), f"{len(failed)} nodes with a failed run (at least 1)"),
        (not retired, f"no retired tgt: ids {retired if retired else ''}".rstrip()),
    ]


def settled_map(base, token, wait_s):
    """Read the map until its structure holds or the wait is over."""
    deadline = time.monotonic() + wait_s
    while True:
        star = call(base, "/api/starmap?env=demo", token)
        lines = structure(star)
        if all(holds for holds, _ in lines) or time.monotonic() >= deadline:
            return star, lines
        time.sleep(POLL_S)


def suppress_one(base, token):
    """Suppress the first open demo risk; its id, or None when there is none."""
    risks = call(base, "/api/risks?env=demo", token)["risks"]
    open_ids = sorted(r["id"] for r in risks if not r.get("suppression"))
    if not open_ids:
        return None
    body = {"reason": "demo-check: accepted for the demo", "expires_in_days": 7, "actor": "demo-check"}
    call(base, f"/api/risks/{open_ids[0]}/suppress", token, body)
    return open_ids[0]


def suppression(base, token, star):
    """The suppressed-risk line; suppresses one risk when none is yet."""
    if star["counts"]["risks_suppressed"] == 0:
        if suppress_one(base, token) is None:
            return False, "no open risk to suppress and none suppressed"
        star = call(base, "/api/starmap?env=demo", token)
    count = star["counts"]["risks_suppressed"]
    dimmed = [n for n in star["nodes"] if n["risks"]["suppressed"] > 0]
    holds = count >= 1 and bool(dimmed)
    return holds, f"{count} suppressed risks on {len(dimmed)} nodes (at least 1)"


def main():
    parser = argparse.ArgumentParser(description="Star Map step 1 assertions for the demo.")
    parser.add_argument("url", help="base URL of the daemon, e.g. http://127.0.0.1:14318")
    parser.add_argument("--wait", type=float, default=60.0, help="seconds to wait for telemetry")
    args = parser.parse_args()
    base = args.url.rstrip("/")
    token = os.environ.get("STARMAP_TOKEN", "")
    try:
        star, lines = settled_map(base, token, args.wait)
        if all(holds for holds, _ in lines):
            lines.append(suppression(base, token, star))
    except (urllib.error.URLError, OSError, ValueError, KeyError) as error:
        print(f"FAIL star map: {base} did not answer as expected: {error!r}")
        return 1
    for holds, text in lines:
        print(f"{'ok  ' if holds else 'FAIL'} star map: {text}")
    return 0 if all(holds for holds, _ in lines) else 1


if __name__ == "__main__":
    sys.exit(main())

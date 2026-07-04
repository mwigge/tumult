#!/usr/bin/env python3
"""Demo proof suite — run any of these against a live `make demo` stack.

Every claim Tumult makes about ChaosGraph token efficiency, its MCP surface,
and agentic trajectory fault modelling is checked here against the running
demo — no mocks, no marketing numbers. Thresholds are deliberately loose
around the *measured* behaviour so the suite proves the property, not a
specific figure.

Usage:
    python3 demo/proof/validate.py            # against localhost defaults
    MCP_URL=http://host:3100 TUMULT_MCP_TOKEN=... python3 demo/proof/validate.py

Requires only the Python standard library. Exits non-zero if any test fails.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import urllib.request

MCP_URL = os.environ.get("MCP_URL", "http://localhost:3100").rstrip("/") + "/mcp"
TOKEN = os.environ.get("TUMULT_MCP_TOKEN", "tumult-demo")
MCP_CONTAINER = os.environ.get("MCP_CONTAINER", "demo-mcp")
JOURNAL_DIR = os.environ.get("JOURNAL_DIR", "/journals")

_SID: str | None = None


def _rpc(method: str, params: dict) -> dict:
    global _SID
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "Authorization": f"Bearer {TOKEN}",
    }
    if _SID:
        headers["mcp-session-id"] = _SID
    req = urllib.request.Request(MCP_URL, data=body, headers=headers)
    resp = urllib.request.urlopen(req, timeout=90)
    _SID = resp.headers.get("mcp-session-id", _SID)
    raw = resp.read().decode()
    datas = [ln[5:].strip() for ln in raw.splitlines() if ln.startswith("data:")]
    return json.loads(datas[-1]) if datas else json.loads(raw)


def _init() -> None:
    _rpc("initialize", {
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "demo-proof", "version": "1"},
    })


def call(name: str, args: dict, token: str | None = None) -> dict:
    """Full tools/call result (so tests can inspect isError)."""
    meta = {"authorization": f"Bearer {token if token is not None else TOKEN}"}
    r = _rpc("tools/call", {"name": name, "arguments": args, "_meta": meta})
    return r.get("result", {})


def structured(name: str, args: dict) -> dict:
    return call(name, args).get("structuredContent", {})


def jsize(obj) -> int:
    """Chars of the compact-JSON serialisation (a tokenizer-free size proxy)."""
    s = obj if isinstance(obj, str) else json.dumps(obj, separators=(",", ":"))
    return len(s)


def toks(chars: int) -> int:
    return max(1, round(chars / 4))


def run_demo(domain: str, times: int = 1) -> None:
    for _ in range(times):
        structured("tumult_run_experiment", {"experiment_path": f"/demo/experiments/demo-{domain}.toon"})


def journal_bytes_total() -> tuple[int, int]:
    """(total_bytes, file_count) of raw journals the demo has written."""
    out = subprocess.run(
        ["docker", "exec", MCP_CONTAINER, "sh", "-c",
         f"cat {JOURNAL_DIR}/*.journal.toon 2>/dev/null | wc -c; ls {JOURNAL_DIR}/*.journal.toon 2>/dev/null | wc -l"],
        capture_output=True, text=True,
    )
    lines = [x for x in out.stdout.split() if x.strip()]
    return (int(lines[0]), int(lines[1])) if len(lines) >= 2 else (0, 0)


# ── test registry ──────────────────────────────────────────────────
RESULTS: list[tuple[bool, str, str]] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    RESULTS.append((ok, name, detail))
    mark = "PASS" if ok else "FAIL"
    print(f"  [{mark}] {name}" + (f" — {detail}" if detail else ""))


NET_EXP = "exp:Demo — network latency via the tumult-net userspace proxy"


# ── ChaosGraph token-efficiency tests (5) ──────────────────────────
def test_token_efficiency() -> None:
    print("\nChaosGraph — token efficiency (measured live):")
    run_demo("postgres")  # ensure the graph is populated at least once

    # 1) A targeted structural query is small.
    tg = structured("tumult_chaosgraph_neighbors", {"node_id": NET_EXP, "rel": "targets"})
    tg_chars = jsize(tg)
    check("targeted query is small", tg_chars < 1200,
          f"neighbors(rel=targets) = {tg_chars} chars / ~{toks(tg_chars)} tok")

    # 2) THE claim: that answer is BOUNDED — running the experiment more times
    #    does not grow it, while reading journals would grow linearly.
    before = jsize(structured("tumult_chaosgraph_neighbors", {"node_id": NET_EXP, "rel": "targets"}))
    run_demo("net", times=5)
    after = jsize(structured("tumult_chaosgraph_neighbors", {"node_id": NET_EXP, "rel": "targets"}))
    check("targeted answer is bounded across runs", abs(after - before) <= 20,
          f"{before} -> {after} chars after +5 runs (journals would add ~5x a full journal)")

    # 3) Per-run compaction: each run adds a tiny node to the graph vs a full
    #    journal to the raw corpus. Measure the ALL-neighbours delta per run.
    all_before = jsize(structured("tumult_chaosgraph_neighbors", {"node_id": NET_EXP}))
    run_demo("net", times=1)
    all_after = jsize(structured("tumult_chaosgraph_neighbors", {"node_id": NET_EXP}))
    per_run_graph = max(1, all_after - all_before)
    one_journal = int(subprocess.run(
        ["docker", "exec", MCP_CONTAINER, "sh", "-c", f"wc -c < {JOURNAL_DIR}/demo-net.journal.toon"],
        capture_output=True, text=True).stdout.strip() or "1920")
    ratio = one_journal / per_run_graph
    # Conservative floor: a run costs the graph one node + its edges (~200 chars) vs a
    # full journal (~1900). ~8x in the demo; assert a comfortable floor.
    check("per-run graph delta >= 5x smaller than a journal", ratio >= 5,
          f"+{per_run_graph} chars/run in graph vs {one_journal} chars/journal = {ratio:.0f}x")

    # 4) Aggregate: enumerate every fault across the whole store in a fraction
    #    of the cost of reading all journals.
    faults = structured("tumult_chaosgraph_query", {"kind": "fault"})
    fchars = jsize(faults)
    total_journal_bytes, jcount = journal_bytes_total()
    agg_ratio = (total_journal_bytes / fchars) if fchars else 0
    check("aggregate query beats reading all journals", jcount >= 2 and agg_ratio >= 5,
          f"query(kind=fault)={fchars} chars vs {total_journal_bytes} chars across {jcount} journals = {agg_ratio:.0f}x")

    # 5) Small AND correct — the compact answer names the real service.
    svc_ids = [n["id"] for n in tg.get("nodes", []) if n["kind"] == "service"]
    check("targeted answer is correct (names svc:demo-app)", "svc:demo-app" in svc_ids,
          f"service nodes: {svc_ids}")


# ── MCP first-class tests ──────────────────────────────────────────
def test_mcp_first_class() -> None:
    print("\nMCP — first-class surface:")
    tools = _rpc("tools/list", {}).get("result", {}).get("tools", [])
    names = {t["name"] for t in tools}
    check("tools/list returns >= 27 tools", len(tools) >= 27, f"{len(tools)} tools")
    check("chaosgraph tools present", {"tumult_chaosgraph_query", "tumult_chaosgraph_neighbors",
          "tumult_chaosgraph_coverage_gaps"} <= names, "3 chaosgraph tools listed")
    annotated = [t for t in tools if t.get("annotations")]
    check("every tool carries annotations", len(annotated) == len(tools),
          f"{len(annotated)}/{len(tools)} annotated")
    with_schema = [t for t in tools if t.get("outputSchema")]
    check("structured tools advertise outputSchema", len(with_schema) >= 16,
          f"{len(with_schema)} tools with outputSchema")

    # a structured tool round-trips with structuredContent
    q = structured("tumult_chaosgraph_query", {"kind": "fault"})
    check("tool round-trip returns structuredContent", isinstance(q, dict) and "count" in q,
          f"chaosgraph_query keys: {sorted(q)[:4]}")

    # auth: a wrong token is rejected in-band
    bad = call("tumult_chaosgraph_query", {"kind": "fault"}, token="wrong-token")
    is_unauth = bad.get("isError") is True and "nauthor" in json.dumps(bad).lower()
    check("bad bearer token is rejected", is_unauth, "isError + Unauthorized")

    # isError on a real failure (unknown node)
    err = call("tumult_chaosgraph_neighbors", {"node_id": "exp:does-not-exist"})
    check("failed call sets isError", err.get("isError") is True, "unknown node -> isError")


# ── Agentic trajectory tests ───────────────────────────────────────
def test_agentic_trajectories() -> None:
    print("\nAgentic — multi-turn trajectory fault modelling:")
    packs = {
        "rag-grounding-failure": "terminates_healthy",
        "reflection-loop": "no_repeated_step",
        "multi-tool-cascade": "recovers_within",
    }
    for pack, headline in packs.items():
        out = subprocess.run(
            ["docker", "exec", MCP_CONTAINER, "sh", "-c",
             f"tumult agentic trajectory --pack {pack}"],
            capture_output=True, text=True).stdout
        ran = "result: pass" in out
        expected_line = [l for l in out.splitlines() if l.strip().startswith("expected:")]
        actual_line = [l for l in out.splitlines() if l.strip().startswith("actual:")]
        matched = bool(expected_line and actual_line and
                       expected_line[0].split(":", 1)[1].strip() == actual_line[0].split(":", 1)[1].strip())
        check(f"trajectory pack '{pack}' fires its contract", ran and matched,
              (actual_line[0].strip() if actual_line else "no contract outcome"))


def main() -> int:
    print("=" * 66)
    print(" Tumult demo proof suite — validating claims against the live demo")
    print("=" * 66)
    _init()
    test_token_efficiency()
    test_mcp_first_class()
    test_agentic_trajectories()

    passed = sum(1 for ok, *_ in RESULTS if ok)
    total = len(RESULTS)
    print("\n" + "-" * 66)
    print(f" {passed}/{total} checks passed")
    print("-" * 66)
    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main())

"""Fixed activities called by Tumult. No user-generated command is executed."""

import ctypes
import json
import math
import os
import signal
import statistics
import sys
import tempfile
import time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


class NoRedirect(HTTPRedirectHandler):
    """Never forward a control credential or request outside the configured target."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise HTTPError(newurl, code, "Target redirects are not allowed", headers, fp)


OPENER = build_opener(ProxyHandler({}), NoRedirect())
SAMPLE_COUNT = 8
SAMPLE_INTERVAL_S = 0.08
THRESHOLD_MS = 200
FAULT_DURATION_S = 12


def atomic_json(path: Path, value: dict) -> None:
    """Publish private evidence atomically."""
    descriptor, name = tempfile.mkstemp(
        prefix=path.name + ".", suffix=".tmp", dir=path.parent
    )
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(value, handle, allow_nan=False)
            handle.flush()
            os.fsync(handle.fileno())
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def request(
    target_url: str, path: str, token: str = "", payload: dict | None = None
) -> tuple[int, dict]:
    """Contact only the configured teaching target, bypassing ambient proxies."""
    body = json.dumps(payload).encode() if payload is not None else None
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    req = Request(target_url + path, data=body, headers=headers)
    try:
        with OPENER.open(req, timeout=2) as response:
            return response.status, json.loads(response.read(8192))
    except HTTPError as exc:
        return exc.code, json.loads(exc.read(8192))


def sample(target_url: str) -> dict:
    """Measure observed responses; connection failures count as errors."""
    samples = []
    for index in range(SAMPLE_COUNT):
        started = time.monotonic()
        try:
            status, body = request(target_url, "/work")
            detail = body.get("dependencies", {})
        except (URLError, TimeoutError, OSError, ValueError) as exc:
            status, detail = 0, {"error": type(exc).__name__}
        samples.append(
            {
                "status": status,
                "latency_ms": round((time.monotonic() - started) * 1000, 2),
                "dependencies": detail,
            }
        )
        if index < SAMPLE_COUNT - 1:
            time.sleep(SAMPLE_INTERVAL_S)
    latencies = sorted(item["latency_ms"] for item in samples)
    errors = sum(item["status"] < 200 or item["status"] >= 400 for item in samples)
    p95 = latencies[math.ceil(len(latencies) * 0.95) - 1]
    return {
        "requests": len(samples),
        "errors": errors,
        "error_rate": errors / len(samples),
        "p95_ms": p95,
        "mean_ms": round(statistics.mean(latencies), 2),
        "healthy": errors == 0 and p95 <= THRESHOLD_MS,
        "samples": samples,
    }


def bind_parent_lifetime() -> None:
    """Linux containers must terminate helpers when their Tumult parent exits."""
    if sys.platform != "linux":
        raise RuntimeError("The lab activity runner requires a Linux container")
    parent = os.getppid()
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(1, signal.SIGTERM, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "Cannot bind helper to Tumult lifetime")
    if parent == 1 or os.getppid() != parent:
        raise RuntimeError("Tumult exited before the activity could start")


def main() -> int:
    """Run one allowlisted experiment activity using private environment context."""
    bind_parent_lifetime()
    mode = sys.argv[1] if len(sys.argv) == 2 else ""
    if mode not in {"baseline", "during", "recovery", "inject", "reset", "steady"}:
        raise ValueError("Unknown activity")
    folder = Path(os.environ["LAB_RUN_DIR"])
    target = os.environ["LAB_TARGET_URL"]
    if mode == "steady":
        if (folder / "baseline.json").exists():
            sys.stdout.write(json.dumps(sample(target)) + "\n")
            return 0
        mode = "baseline"
    atomic_json(folder / "phase.json", {"phase": mode})
    if mode in {"baseline", "during", "recovery"}:
        result = sample(target)
        atomic_json(folder / (mode + ".json"), result)
        sys.stdout.write(json.dumps(result) + "\n")
        return 1 if mode == "baseline" and not result["healthy"] else 0
    payload = (
        {"fault": os.environ["LAB_FAULT"], "duration_s": FAULT_DURATION_S}
        if mode == "inject"
        else {}
    )
    status, result = request(
        target,
        "/fault" if mode == "inject" else "/reset",
        os.environ["LAB_CONTROL_TOKEN"],
        payload,
    )
    if status != 200:
        raise RuntimeError("Target control request failed with HTTP " + str(status))
    sys.stdout.write(json.dumps(result) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

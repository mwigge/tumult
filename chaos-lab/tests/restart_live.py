"""Crash the lab service mid-fault, prove independent expiry, then restart it."""

import json
import subprocess
import time
from pathlib import Path

import httpx

ROOT = Path(__file__).parents[1]


def compose(*arguments: str, input: str | None = None) -> str:
    result = subprocess.run(
        ["docker", "compose", *arguments],
        cwd=ROOT,
        input=input,
        capture_output=True,
        text=True,
        timeout=120,
        check=True,
    )
    return result.stdout


def main() -> None:
    with httpx.Client(
        base_url="http://127.0.0.1:8089", trust_env=False, timeout=10
    ) as client:
        client.get("/api/status").raise_for_status()
        saved = client.get("/api/runs").json()
        completed = {run["id"] for run in saved if run["status"] == "completed"}
        assert completed, "Run the browser test first to establish persistent evidence."
        client.post(
            "/api/settings", json={"provider": "ollama", "model": "llama3.2:3b"}
        ).raise_for_status()
        response = client.post(
            "/api/runs", json={"scenario_id": "latency", "armed": True}
        )
        response.raise_for_status()
        run_id = response.json()["id"]
        for _ in range(100):
            run = client.get(f"/api/runs/{run_id}").json()
            if run["phase"] == "during":
                break
            time.sleep(0.1)
        else:
            raise AssertionError("Fault did not become active.")
        compose("stop", "--timeout", "0", "lab")
        try:
            observation = compose(
                "exec",
                "-T",
                "target",
                "python",
                "-",
                input="""
import json,time,urllib.request
started=time.monotonic()
def get(path):
    with urllib.request.urlopen('http://127.0.0.1:5000'+path,timeout=3) as response:
        return json.load(response)
first=get('/health')['fault']
assert first == 'latency', first
while get('/health')['fault'] is not None:
    assert time.monotonic()-started < 16
    time.sleep(.2)
assert get('/work')['status'] == 'ok'
print(json.dumps({'fault_after_lab_crash':first,'expiry_seconds_after_crash':round(time.monotonic()-started,2),'target_recovered_without_lab':True}))
""",
            )
        finally:
            compose("up", "-d", "--wait", "--wait-timeout", "120")
        status = client.get("/api/status").json()
        assert status["engine"]["available"] and status["target"]["healthy"]
        assert not status["provider"]["configured"]
        interrupted = client.get(f"/api/runs/{run_id}").json()
        assert interrupted["status"] == "interrupted"
        assert interrupted["verdict"] == "inconclusive"
        assert interrupted["recovery"] is None
        after = client.get("/api/runs").json()
        assert completed <= {run["id"] for run in after}
        result = json.loads(observation)
        result.update(
            interrupted_run=run_id,
            saved_completed_runs=len(completed),
            credentials_cleared=True,
        )
        (ROOT / "test-results" / "restart-result.json").write_text(
            json.dumps(result, indent=2)
        )


if __name__ == "__main__":
    main()

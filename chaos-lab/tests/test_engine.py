import asyncio
import json
from pathlib import Path

import pytest

from agent.engine import LabEngine


def test_catalogue_is_bounded_and_has_no_arbitrary_commands(tmp_path):
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:5000", "test-token", "/missing/tumult"
    )
    scenarios = engine.scenarios()
    assert len(scenarios) >= 4
    assert all(0 < item["duration_s"] <= 15 for item in scenarios)
    with pytest.raises(ValueError):
        asyncio.run(engine.start("../../etc/passwd"))
    assert engine.journal_path("../../etc/passwd") is None


def test_saved_interrupted_run_is_marked_without_inventing_metrics(tmp_path):
    run_id = "a" * 32
    folder = tmp_path / run_id
    folder.mkdir()
    (folder / "run.json").write_text(
        json.dumps({"id": run_id, "status": "running", "events": []})
    )
    engine = LabEngine(tmp_path, "http://127.0.0.1:1", "test-token", "/missing/tumult")

    async def check():
        await engine.initialize()
        result = await engine.get(run_id)
        assert result["status"] == "interrupted"
        assert not result.get("recovery")
        assert result["events"]

    asyncio.run(check())


@pytest.mark.parametrize(
    "scenario_id", ["latency", "http-errors", "cache-outage", "db-timeout"]
)
def test_real_tumult_execution_and_early_cancellation(tmp_path, scenario_id):
    import importlib.util
    import os
    import threading

    binary = os.environ.get("TUMULT_TEST_BIN")
    if not binary:
        pytest.skip("Set TUMULT_TEST_BIN to exercise the actual Tumult release binary")
    spec = importlib.util.spec_from_file_location(
        "integration_target", Path(__file__).parents[1] / "targets/api/app.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    token = "private-integration-control-token"
    server = module.make_server("127.0.0.1", 0, token)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    engine = LabEngine(
        tmp_path, f"http://127.0.0.1:{server.server_port}", token, binary
    )

    async def check():
        await engine.initialize()
        assert (await engine.health())["tumult"]
        run = await engine.start(scenario_id)
        with pytest.raises(RuntimeError):
            await engine.start("cache-outage")
        for _ in range(300):
            result = await engine.get(run["id"])
            if not engine.active_run_id:
                break
            await asyncio.sleep(0.1)
        assert result["status"] == "completed", result
        assert result["verdict"] == "deviated"
        assert result["baseline"]["healthy"]
        if scenario_id == "latency":
            assert result["during"]["p95_ms"] >= 350
        else:
            assert result["during"]["errors"] == result["during"]["requests"]
        if scenario_id == "db-timeout":
            assert result["during"]["p95_ms"] >= 550
        assert result["recovery"]["healthy"]
        assert result["journal_available"]
        assert "status: completed" in engine.journal_path(run["id"]).read_text()
        assert result["native_status"] == "completed"
        assert token not in engine.journal_path(run["id"]).read_text()
        assert token not in engine.experiment_path(run["id"]).read_text()
        stopped = await engine.start("db-timeout")
        result = await engine.stop(stopped["id"])
        assert result["status"] == "stopped"
        assert engine.active_run_id is None
        assert result["recovery"]["healthy"]
        if scenario_id == "latency":
            active = await engine.start("db-timeout")
            for _ in range(100):
                if (await engine.get(active["id"]))["phase"] == "during":
                    break
                await asyncio.sleep(0.02)
            else:
                pytest.fail("The injected fault never became active")
            result = await engine.stop(active["id"])
            assert result["status"] == "stopped"
            assert result["recovery"]["healthy"]
            assert result["during"] is None
            assert not engine.active_run_id
            broken = LabEngine(
                tmp_path / "unavailable", engine.target_url, token, "/missing/tumult"
            )
            assert not (await broken.health())["tumult"]
            failed = await broken.start("latency")
            for _ in range(100):
                if not broken.active_run_id:
                    break
                await asyncio.sleep(0.02)
            failure = await broken.get(failed["id"])
            assert failure["status"] == "failed"
            assert failure["recovery"]["healthy"]
            assert failure["baseline"] is None
            restored = LabEngine(tmp_path, engine.target_url, token, binary)
            assert (await restored.get(run["id"]))["verdict"] == "deviated"
        await engine.close()

    try:
        asyncio.run(check())
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


@pytest.mark.parametrize(
    "url",
    [
        "https://example.com",
        "http://169.254.169.254",
        "http://api/work",
        "http://user:pass@api",
        "http://api?target=x",
    ],
)
def test_engine_rejects_arbitrary_targets(tmp_path, url):
    with pytest.raises(ValueError):
        LabEngine(tmp_path, url, "private-token", "/missing/tumult")


def test_engine_storage_limit_preserves_existing_evidence(tmp_path):
    from agent.engine import MAX_RUNS

    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )
    engine._records = {str(index): {} for index in range(MAX_RUNS)}
    with pytest.raises(RuntimeError, match="History limit"):
        asyncio.run(engine.start("latency"))
    assert not list(tmp_path.iterdir())


def test_target_requests_do_not_follow_redirects():
    from urllib.error import HTTPError

    from agent.lab_activity import NoRedirect

    with pytest.raises(HTTPError):
        NoRedirect().redirect_request(
            None, None, 302, "Found", {}, "http://example.com"
        )


def test_corrupt_stage_is_reported_as_failed_evidence(tmp_path):
    run_id = "b" * 32
    folder = tmp_path / run_id
    folder.mkdir()
    (folder / "run.json").write_text(
        json.dumps({"id": run_id, "status": "completed", "events": []})
    )
    (folder / "during.json").write_text("{broken")
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )
    result = asyncio.run(engine.get(run_id))
    assert result["status"] == "failed"
    assert result["verdict"] == "inconclusive"
    assert result["during"] is None


def test_unavailable_target_releases_active_run_and_preserves_failure(tmp_path):
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )

    async def check():
        run = await engine.start("latency")
        for _ in range(100):
            if not engine.active_run_id:
                break
            await asyncio.sleep(0.01)
        result = await engine.get(run["id"])
        assert result["status"] == "failed"
        assert result["verdict"] == "inconclusive"
        assert result["recovery"] is None
        assert "expires automatically" in result["explanation"]
        assert engine.active_run_id is None

    asyncio.run(check())


def test_atomic_evidence_keeps_previous_file_when_serialization_fails(tmp_path):
    from agent.lab_activity import atomic_json

    path = tmp_path / "evidence.json"
    atomic_json(path, {"healthy": True})
    with pytest.raises(ValueError):
        atomic_json(path, {"latency": float("nan")})
    assert json.loads(path.read_text()) == {"healthy": True}
    assert path.stat().st_mode & 0o777 == 0o600
    assert not list(tmp_path.glob("*.tmp"))


def test_load_skips_invalid_folders_and_corrupt_records(tmp_path):
    for name in ["not-an-id", "c" * 32]:
        folder = tmp_path / name
        folder.mkdir()
        (folder / "run.json").write_text("{broken")
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )
    assert asyncio.run(engine.list_runs()) == []
    assert asyncio.run(engine.get("missing")) is None
    assert asyncio.run(engine.stop("missing")) is None


@pytest.mark.parametrize("phase", [{"phase": "unknown"}, [], "{invalid"])
def test_invalid_live_phase_does_not_invent_progress(tmp_path, phase):
    run_id = "d" * 32
    folder = tmp_path / run_id
    folder.mkdir()
    (folder / "run.json").write_text(
        json.dumps(
            {"id": run_id, "status": "running", "phase": "preparing", "events": []}
        )
    )
    (folder / "phase.json").write_text(
        phase if isinstance(phase, str) else json.dumps(phase)
    )
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )
    assert asyncio.run(engine.get(run_id))["phase"] == "preparing"


def test_invalid_metrics_shape_fails_closed(tmp_path):
    run_id = "e" * 32
    folder = tmp_path / run_id
    folder.mkdir()
    (folder / "run.json").write_text(
        json.dumps({"id": run_id, "status": "completed", "events": []})
    )
    (folder / "during.json").write_text('{"healthy": true, "p95_ms": -1}')
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )
    assert asyncio.run(engine.get(run_id))["verdict"] == "inconclusive"


def test_health_timeout_reaps_process_and_reports_target_failure(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from unittest.mock import AsyncMock, Mock

    from agent import engine as module

    process = SimpleNamespace(
        communicate=AsyncMock(side_effect=TimeoutError),
        wait=AsyncMock(return_value=0),
        kill=Mock(),
    )
    monkeypatch.setattr(
        module.asyncio, "create_subprocess_exec", AsyncMock(return_value=process)
    )
    monkeypatch.setattr(module, "request", Mock(side_effect=OSError("offline")))
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )
    result = asyncio.run(engine.health())
    assert result["tumult"] is False and result["target"] is False
    process.kill.assert_called_once()
    process.wait.assert_awaited_once()


@pytest.mark.parametrize("disappears", ["first-signal", "kill-signal", "never"])
def test_termination_handles_exit_races_and_escalates_only_after_timeout(
    tmp_path, monkeypatch, disappears
):
    import signal
    from types import SimpleNamespace
    from unittest.mock import AsyncMock, Mock

    from agent import engine as module

    wait = AsyncMock(side_effect=[TimeoutError(), 0])
    process = SimpleNamespace(pid=123456, returncode=None, wait=wait)
    signals = Mock(
        side_effect=(
            [ProcessLookupError()]
            if disappears == "first-signal"
            else [None, ProcessLookupError()] if disappears == "kill-signal" else None
        )
    )
    monkeypatch.setattr(module.os, "killpg", signals)
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )
    engine._process = process
    asyncio.run(engine._terminate())
    assert signals.call_args_list[0].args == (123456, signal.SIGINT)
    if disappears == "first-signal":
        wait.assert_not_called()
    else:
        assert signals.call_args_list[1].args == (123456, signal.SIGKILL)
        assert wait.await_count == 2


@pytest.mark.parametrize(
    "native_status,returncode,recovery_healthy,missing,expected",
    [
        ("failed", 1, True, False, "failed"),
        ("completed", 8, True, False, "failed"),
        ("completed", 0, True, True, "failed"),
        ("completed", 0, False, False, "inconclusive"),
        ("completed", 0, True, False, "held"),
    ],
)
def test_native_outcome_and_required_evidence_gate_verdict(
    tmp_path,
    monkeypatch,
    native_status,
    returncode,
    recovery_healthy,
    missing,
    expected,
):
    from types import SimpleNamespace
    from unittest.mock import AsyncMock

    from agent import engine as module

    metrics = {
        "healthy": True,
        "requests": 8,
        "errors": 0,
        "error_rate": 0,
        "p95_ms": 1,
        "mean_ms": 1,
    }
    process = SimpleNamespace(
        returncode=returncode, wait=AsyncMock(return_value=returncode)
    )

    async def launch(*args, **kwargs):
        folder = kwargs["cwd"]
        for stage in ("baseline", "during", "recovery"):
            if missing and stage == "during":
                continue
            value = dict(
                metrics, healthy=recovery_healthy if stage == "recovery" else True
            )
            (folder / (stage + ".json")).write_text(json.dumps(value))
        (folder / "journal.toon").write_text("status: " + native_status)
        return process

    monkeypatch.setattr(module.asyncio, "create_subprocess_exec", launch)
    monkeypatch.setattr(module, "request", lambda *args: (200, {}))
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )

    async def check():
        run = await engine.start("latency")
        await engine._task
        result = await engine.get(run["id"])
        assert result["status"] == ("failed" if expected == "failed" else "completed")
        assert result["verdict"] == (
            "inconclusive" if expected == "failed" else expected
        )
        assert engine.active_run_id is None
        history = await engine.list_runs()
        history[0]["title"] = "altered"
        assert (await engine.get(run["id"]))["title"] != "altered"

    asyncio.run(check())


def test_reset_rejection_aborts_start_and_cannot_claim_cleanup(tmp_path, monkeypatch):
    from agent import engine as module

    monkeypatch.setattr(module, "request", lambda *args: (401, {}))
    engine = LabEngine(
        tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
    )

    async def check():
        run = await engine.start("latency")
        await engine._task
        result = await engine.get(run["id"])
        assert result["status"] == "failed"
        assert not result["baseline"] and not result["recovery"]
        assert any("Reset failed" in event["message"] for event in result["events"])

    asyncio.run(check())


def test_cancellation_during_process_launch_waits_for_handle_then_reaps(
    tmp_path, monkeypatch
):
    from types import SimpleNamespace
    from unittest.mock import AsyncMock, Mock

    from agent import engine as module

    metrics = {
        "healthy": True,
        "requests": 8,
        "errors": 0,
        "error_rate": 0,
        "p95_ms": 1,
        "mean_ms": 1,
    }

    async def check():
        entered, release = asyncio.Event(), asyncio.Event()
        process = SimpleNamespace(
            pid=123456, returncode=None, wait=AsyncMock(return_value=0)
        )

        async def launch(*args, **kwargs):
            entered.set()
            await release.wait()
            return process

        signals = Mock()
        monkeypatch.setattr(module.asyncio, "create_subprocess_exec", launch)
        monkeypatch.setattr(module.os, "killpg", signals)
        monkeypatch.setattr(module, "request", lambda *args: (200, {}))
        monkeypatch.setattr(module, "sample", lambda *args: metrics)
        engine = LabEngine(
            tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
        )
        run = await engine.start("latency")
        await entered.wait()
        closing = asyncio.create_task(engine.close())
        await asyncio.sleep(0)
        release.set()
        await closing
        assert signals.call_count == 1
        assert (await engine.get(run["id"]))["status"] == "stopped"
        assert engine.active_run_id is None

    asyncio.run(check())


def test_custom_path_binary_remains_executable_with_sanitized_environment(
    tmp_path, monkeypatch
):
    import os
    import sys

    from agent import engine as module

    binary_dir = tmp_path / "custom-bin"
    binary_dir.mkdir()
    binary = binary_dir / "test-tumult"
    binary.write_text(
        "#!"
        + sys.executable
        + '\nimport os, sys\nassert "OPENAI_API_KEY" not in os.environ or "--version" in sys.argv\nprint("custom-location-executed")\n'
    )
    binary.chmod(0o700)
    monkeypatch.setenv("PATH", str(binary_dir) + os.pathsep + os.environ["PATH"])
    monkeypatch.setenv("OPENAI_API_KEY", "test-key-must-not-reach-runner")
    monkeypatch.setattr(module, "request", lambda *args: (200, {}))
    monkeypatch.setattr(
        module,
        "sample",
        lambda *args: {
            "healthy": True,
            "requests": 8,
            "errors": 0,
            "error_rate": 0,
            "p95_ms": 1,
            "mean_ms": 1,
        },
    )
    engine = LabEngine(
        tmp_path / "evidence", "http://127.0.0.1:1", "private-token", "test-tumult"
    )

    async def check():
        assert (await engine.health())["tumult"]
        run = await engine.start("latency")
        await engine._task
        result = await engine.get(run["id"])
        assert (
            "custom-location-executed"
            in (engine.data_dir / run["id"] / "tumult.log").read_text()
        )
        assert "did not produce a complete experiment" in result["explanation"]

    asyncio.run(check())


@pytest.mark.parametrize("outcome", ["completed", "stopped"])
def test_terminal_status_waits_for_cleanup_and_final_evidence(
    tmp_path, monkeypatch, outcome
):
    from types import SimpleNamespace
    from unittest.mock import AsyncMock

    from agent import engine as module

    metrics = {
        "healthy": True,
        "requests": 8,
        "errors": 0,
        "error_rate": 0,
        "p95_ms": 1,
        "mean_ms": 1,
    }

    async def check():
        cleanup_entered, cleanup_release, process_entered = (
            asyncio.Event(),
            asyncio.Event(),
            asyncio.Event(),
        )

        async def wait():
            process_entered.set()
            if outcome == "stopped":
                await asyncio.Event().wait()
            return 0

        process = SimpleNamespace(returncode=0, wait=wait)

        async def launch(*args, **kwargs):
            if outcome == "completed":
                for stage in ("baseline", "during", "recovery"):
                    (kwargs["cwd"] / (stage + ".json")).write_text(json.dumps(metrics))
                (kwargs["cwd"] / "journal.toon").write_text("status: completed")
            return process

        engine = LabEngine(
            tmp_path, "http://127.0.0.1:1", "private-token", "/missing/tumult"
        )
        calls = 0

        async def reset():
            nonlocal calls
            calls += 1
            if calls == 2:
                cleanup_entered.set()
                await cleanup_release.wait()

        monkeypatch.setattr(engine, "_reset", reset)
        monkeypatch.setattr(engine, "_terminate", AsyncMock())
        monkeypatch.setattr(module.asyncio, "create_subprocess_exec", launch)
        monkeypatch.setattr(module, "sample", lambda *args: metrics)
        run = await engine.start("latency")
        await process_entered.wait()
        stopping = (
            asyncio.create_task(engine.stop(run["id"]))
            if outcome == "stopped"
            else None
        )
        await cleanup_entered.wait()
        repeated_stop = asyncio.create_task(engine.stop(run["id"]))
        await asyncio.sleep(0)
        try:
            assert not repeated_stop.done()
            assert engine._task.cancelling() == (1 if outcome == "stopped" else 0)
            pending = await engine.get(run["id"])
            assert pending["status"] in {"running", "stopping", "finalizing"}
            assert pending["finished_at"] is None
            assert engine.active_run_id == run["id"]
            saved = json.loads((tmp_path / run["id"] / "run.json").read_text())
            assert saved["status"] in {"running", "stopping", "finalizing"}
        finally:
            cleanup_release.set()
            await (stopping or engine._task)
            await repeated_stop
        finished = await engine.get(run["id"])
        assert finished["status"] == outcome
        assert finished["finished_at"] is not None
        assert finished["recovery"]["healthy"]
        assert engine.active_run_id is None

    asyncio.run(check())

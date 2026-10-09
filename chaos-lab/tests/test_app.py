"""Local browser boundary and provider-key isolation contracts."""

import asyncio
import json
from pathlib import Path
from unittest.mock import AsyncMock, Mock

import httpx
import pytest
from fastapi.testclient import TestClient

from agent.app import COOKIE, create_app
from agent.providers import ProviderError


@pytest.fixture
def lab(tmp_path: Path):
    engine = AsyncMock()
    engine.scenarios = lambda: [{"id": "latency", "title": "Latency"}]
    engine.health.return_value = {"available": True, "version": "tumult 2.22.0"}
    engine.list_runs.return_value = []
    engine.start.return_value = {"id": "a-run", "status": "running"}
    app = create_app(engine=engine, web_dir=tmp_path)
    with TestClient(app, base_url="http://localhost:8089") as client:
        yield client, engine, app


def session(client):
    response = client.get("/api/status")
    assert response.status_code == 200
    return response


def test_session_is_private_and_security_headers_apply(lab):
    client, _, _ = lab
    result = session(client)
    assert "httponly" in result.headers["set-cookie"].lower()
    assert "samesite=strict" in result.headers["set-cookie"].lower()
    assert result.headers["cache-control"] == "no-store"
    assert result.headers["x-content-type-options"] == "nosniff"
    assert result.json()["provider"]["configured"] is False


def test_fault_run_requires_existing_session_and_explicit_arming(lab):
    client, engine, _ = lab
    assert (
        client.post(
            "/api/runs", json={"scenario_id": "latency", "armed": True}
        ).status_code
        == 403
    )
    session(client)
    assert client.post("/api/runs", json={"scenario_id": "latency"}).status_code == 403
    assert (
        client.post(
            "/api/runs", json={"scenario_id": "latency", "armed": "true"}
        ).status_code
        == 422
    )
    engine.start.assert_not_called()
    result = client.post("/api/runs", json={"scenario_id": "latency", "armed": True})
    assert result.status_code == 202
    engine.start.assert_awaited_once_with("latency")


@pytest.mark.parametrize(
    "headers", [{"Origin": "https://evil.example"}, {"Sec-Fetch-Site": "cross-site"}]
)
def test_cross_origin_writes_are_rejected(lab, headers):
    client, engine, _ = lab
    session(client)
    result = client.post(
        "/api/runs", json={"scenario_id": "latency", "armed": True}, headers=headers
    )
    assert result.status_code == 403
    engine.start.assert_not_called()


def test_dns_rebinding_and_oversize_bodies_are_rejected(lab):
    client, _, _ = lab
    assert (
        client.get("/api/status", headers={"Host": "attacker.example"}).status_code
        == 400
    )
    session(client)
    result = client.post("/api/ask", json={"q": "x" * 40000})
    assert result.status_code == 413


def test_provider_keys_are_not_returned_or_shared_with_another_session(lab):
    client, _, _ = lab
    session(client)
    result = client.post(
        "/api/settings",
        json={
            "provider": "openai",
            "model": "gpt-4.1-mini",
            "api_key": "sk-private-test",
        },
    )
    assert result.status_code == 200
    assert "sk-private-test" not in result.text
    assert result.json()["configured"] is True
    assert "sk-private-test" not in client.get("/api/status").text
    client.cookies.clear()
    assert session(client).json()["provider"]["configured"] is False


def test_validation_errors_do_not_echo_secrets(lab):
    client, _, _ = lab
    session(client)
    secret = "sk-private-should-never-return"
    result = client.post(
        "/api/settings", json={"provider": "invalid", "model": "x", "api_key": secret}
    )
    assert result.status_code == 422
    assert secret not in result.text
    result = client.post(
        "/api/settings",
        json={"provider": "openai", "model": "x", "api_key": secret * 300},
    )
    assert result.status_code == 422
    assert secret not in result.text


def test_ollama_needs_no_key_and_forget_clears_provider(lab):
    client, _, _ = lab
    session(client)
    result = client.post(
        "/api/settings",
        json={"provider": "ollama", "model": "llama3.2:3b", "api_key": ""},
    )
    assert result.status_code == 200
    assert result.json()["configured"] is True
    assert client.delete("/api/settings").status_code == 200
    assert client.get("/api/status").json()["provider"]["configured"] is False


def test_tutor_needs_key_and_cannot_start_faults(lab, monkeypatch):
    client, engine, _ = lab
    session(client)
    assert client.post("/api/ask", json={"q": "explain latency"}).status_code == 409
    client.post(
        "/api/settings",
        json={"provider": "ollama", "model": "llama3.2:3b", "api_key": ""},
    )
    tutor = AsyncMock(return_value="Measure your baseline first.")
    monkeypatch.setattr("agent.app.ask_provider", tutor)
    response = client.post("/api/ask", json={"q": "inject every possible fault"})
    assert response.status_code == 200
    assert response.json()["answer"] == "Measure your baseline first."
    engine.start.assert_not_called()


def test_busy_and_unknown_experiments_return_actionable_status(lab):
    client, engine, _ = lab
    session(client)
    engine.start.side_effect = ValueError("Unknown scenario")
    assert (
        client.post(
            "/api/runs", json={"scenario_id": "evil", "armed": True}
        ).status_code
        == 400
    )
    engine.start.side_effect = RuntimeError("Another run is active")
    assert (
        client.post(
            "/api/runs", json={"scenario_id": "latency", "armed": True}
        ).status_code
        == 409
    )


def test_no_arbitrary_extra_parameters_reach_engine(lab):
    client, engine, _ = lab
    session(client)
    result = client.post(
        "/api/runs",
        json={
            "scenario_id": "latency",
            "armed": True,
            "target": "http://production",
            "command": "rm -rf /",
        },
    )
    assert result.status_code == 422
    engine.start.assert_not_called()


def configure_tutor(client):
    session(client)
    response = client.post(
        "/api/settings", json={"provider": "ollama", "model": "llama3.2:3b"}
    )
    assert response.status_code == 200


@pytest.mark.parametrize(
    "secret", ["bad\nsecret", "bad\rsecret", "bad\x00secret", "secret 🔑", "a b"]
)
def test_corrupt_secret_rejected_without_echo_or_overwriting_settings(lab, secret):
    client, _, _ = lab
    configure_tutor(client)
    response = client.post(
        "/api/settings",
        json={"provider": "openai", "model": "gpt-4.1-mini", "api_key": secret},
    )
    assert response.status_code == 400
    assert secret not in response.text
    assert client.get("/api/status").json()["provider"]["provider"] == "ollama"


def test_malformed_json_validation_never_echoes_secret(lab):
    client, _, _ = lab
    session(client)
    response = client.post(
        "/api/settings",
        content='{"api_key":"sk-private-invalid-json",',
        headers={"content-type": "application/json"},
    )
    assert response.status_code == 422
    assert "sk-private-invalid-json" not in response.text


def test_independent_sessions_use_and_forget_only_their_own_keys(lab, monkeypatch):
    client, _, _ = lab
    session(client)
    first_sid = client.cookies.get(COOKIE)
    client.post(
        "/api/settings",
        json={"provider": "openai", "model": "gpt-4.1-mini", "api_key": "first-secret"},
    )
    client.cookies.clear()
    session(client)
    second_sid = client.cookies.get(COOKIE)
    assert first_sid != second_sid
    client.post(
        "/api/settings",
        json={
            "provider": "anthropic",
            "model": "claude-sonnet-4-6",
            "api_key": "second-secret",
        },
    )
    tutor = AsyncMock(return_value="Measured evidence.")
    monkeypatch.setattr("agent.app.ask_provider", tutor)
    assert client.post("/api/ask", json={"q": "explain"}).status_code == 200
    assert tutor.call_args.args[0].api_key == "second-secret"
    client.delete("/api/settings")
    client.cookies.clear()
    client.cookies.set(COOKIE, first_sid)
    assert client.post("/api/ask", json={"q": "explain"}).status_code == 200
    assert tutor.call_args.args[0].api_key == "first-secret"


def test_session_expiry_discards_provider_credentials(lab, monkeypatch):
    client, _, _ = lab
    configure_tutor(client)
    original_sid = client.cookies.get(COOKIE)
    monkeypatch.setattr("agent.app.SESSION_TTL", -1)
    assert client.post("/api/ask", json={"q": "explain"}).status_code == 403
    state = session(client)
    assert state.json()["provider"]["configured"] is False
    assert client.cookies.get(COOKIE) != original_sid


def test_tutor_rate_limit_is_per_session_and_expires(lab, monkeypatch):
    client, _, _ = lab
    configure_tutor(client)
    tutor = AsyncMock(return_value="Measure the baseline.")
    monkeypatch.setattr("agent.app.ask_provider", tutor)
    for _ in range(10):
        assert client.post("/api/ask", json={"q": "explain"}).status_code == 200
    assert client.post("/api/ask", json={"q": "explain"}).status_code == 429
    assert tutor.await_count == 10
    client.cookies.clear()
    configure_tutor(client)
    assert client.post("/api/ask", json={"q": "explain"}).status_code == 200


@pytest.mark.asyncio
@pytest.mark.parametrize("wait_on_evidence", [False, True])
async def test_concurrent_tutor_requests_rejected_before_paid_call(
    lab, monkeypatch, wait_on_evidence
):
    _, engine, app = lab
    entered = asyncio.Event()
    release = asyncio.Event()
    calls = 0

    async def block_once(*_args):
        nonlocal calls
        calls += 1
        if calls == 1:
            entered.set()
            await release.wait()
        return (
            {"id": "run-one", "status": "completed"}
            if wait_on_evidence
            else "Evidence explanation."
        )

    tutor = AsyncMock(return_value="Evidence explanation.")
    if wait_on_evidence:
        engine.get.side_effect = block_once
    else:
        tutor.side_effect = block_once
    monkeypatch.setattr("agent.app.ask_provider", tutor)
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://localhost:8089"
    ) as client:
        await client.get("/api/status")
        await client.post(
            "/api/settings", json={"provider": "ollama", "model": "llama3.2:3b"}
        )
        body = {"q": "explain"}
        if wait_on_evidence:
            body["run_id"] = "run-one"
        pending = asyncio.create_task(client.post("/api/ask", json=body))
        try:
            await asyncio.wait_for(entered.wait(), timeout=2)
            second = await asyncio.wait_for(
                client.post("/api/ask", json=body), timeout=2
            )
        finally:
            release.set()
            first = await asyncio.wait_for(pending, timeout=2)
        assert first.status_code == 200
        assert second.status_code == 429
        assert tutor.await_count == 1


def test_failed_tutor_releases_lock_and_returns_safe_error(lab, monkeypatch):
    client, _, _ = lab
    configure_tutor(client)
    tutor = AsyncMock(
        side_effect=[
            ProviderError("Provider temporarily unavailable.", 503),
            "Measured baseline.",
        ]
    )
    monkeypatch.setattr("agent.app.ask_provider", tutor)
    failed = client.post("/api/ask", json={"q": "explain"})
    assert failed.status_code == 503
    assert failed.json() == {"detail": "Provider temporarily unavailable."}
    assert client.post("/api/ask", json={"q": "explain"}).status_code == 200


@pytest.mark.parametrize("suffix", ["", "/export", "/journal", "/stop"])
def test_unknown_run_evidence_and_controls_are_not_exposed(lab, suffix):
    client, engine, _ = lab
    session(client)
    engine.get.return_value = None
    if suffix == "/stop":
        result = client.post("/api/runs/unknown/stop", json={})
    else:
        result = client.get("/api/runs/unknown" + suffix)
    assert result.status_code == 404
    engine.stop.assert_not_called()


def test_measured_run_is_sent_to_tutor_and_exported_without_invented_evidence(
    lab, monkeypatch
):
    client, engine, _ = lab
    configure_tutor(client)
    evidence = {
        "id": "run-one",
        "status": "completed",
        "baseline": {"latency_ms": 11},
        "fault": {"latency_ms": 417},
        "recovery": {"latency_ms": 12},
    }
    engine.get.return_value = evidence
    tutor = AsyncMock(return_value="The fault increased measured latency.")
    monkeypatch.setattr("agent.app.ask_provider", tutor)
    result = client.post("/api/ask", json={"q": "what happened", "run_id": "run-one"})
    assert result.status_code == 200
    assert tutor.call_args.args[2]["run"] == evidence
    exported = client.get("/api/runs/run-one/export")
    assert exported.json() == {"format": "tumult-chaos-lab/v1", "run": evidence}
    assert (
        exported.headers["content-disposition"]
        == 'attachment; filename="chaos-lab-run-one.json"'
    )


def test_unknown_evidence_never_calls_provider(lab, monkeypatch):
    client, engine, _ = lab
    configure_tutor(client)
    engine.get.return_value = None
    tutor = AsyncMock()
    monkeypatch.setattr("agent.app.ask_provider", tutor)
    assert (
        client.post(
            "/api/ask", json={"q": "what happened", "run_id": "unknown"}
        ).status_code
        == 404
    )
    tutor.assert_not_called()


def test_journal_download_requires_existing_evidence_file(lab, tmp_path):
    client, engine, _ = lab
    session(client)
    engine.get.return_value = {"id": "run-one"}
    path = tmp_path / "journal.toon"
    engine.journal_path = lambda _: path
    assert client.get("/api/runs/run-one/journal").status_code == 404
    path.write_text("measured-journal", encoding="utf-8")
    result = client.get("/api/runs/run-one/journal")
    assert result.status_code == 200
    assert result.text == "measured-journal"
    assert (
        result.headers["content-disposition"]
        == 'attachment; filename="tumult-run-one.toon"'
    )


def test_content_security_policy_and_same_origin_csrf_boundary(lab):
    client, engine, _ = lab
    result = session(client)
    csp = result.headers["content-security-policy"]
    for rule in [
        "script-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "frame-ancestors 'none'",
        "connect-src 'self'",
    ]:
        assert rule in csp
    assert "unsafe-inline" not in csp
    assert "unsafe-eval" not in csp
    assert (
        client.post(
            "/api/runs",
            json={"scenario_id": "latency", "armed": True},
            headers={"Origin": "http://localhost:8089"},
        ).status_code
        == 202
    )
    engine.start.reset_mock()
    for origin in [
        "null",
        "http://localhost:9999",
        "http://127.0.0.1:8089",
        "https://localhost:8089",
    ]:
        assert (
            client.post(
                "/api/runs",
                json={"scenario_id": "latency", "armed": True},
                headers={"Origin": origin},
            ).status_code
            == 403
        )
    engine.start.assert_not_called()


def test_declared_body_size_cannot_bypass_actual_limit(lab):
    client, _, _ = lab
    session(client)
    payload = json.dumps(
        {"provider": "openai", "model": "gpt-4.1-mini", "api_key": "s" * 40000}
    )
    result = client.post(
        "/api/settings",
        content=payload,
        headers={"content-type": "application/json", "content-length": "1"},
    )
    assert result.status_code == 413


def test_health_probes_do_not_consume_browser_sessions(lab, monkeypatch):
    client, _, _ = lab
    monkeypatch.setattr("agent.app.MAX_SESSIONS", 1)
    for _ in range(4):
        client.cookies.clear()
        response = client.get("/api/health")
        assert response.status_code == 200
        assert "set-cookie" not in response.headers
    assert session(client).status_code == 200


def test_experiment_definition_download_is_native_and_missing_file_is_explicit(
    lab, tmp_path
):
    client, engine, _ = lab
    session(client)
    engine.get.return_value = {"id": "run-one"}
    path = tmp_path / "experiment.toon"
    engine.experiment_path = lambda _: path
    assert client.get("/api/runs/run-one/experiment").status_code == 404
    path.write_text("title: bounded experiment\n")
    response = client.get("/api/runs/run-one/experiment")
    assert response.status_code == 200
    assert response.text == "title: bounded experiment\n"
    assert (
        response.headers["content-disposition"]
        == 'attachment; filename="experiment-run-one.toon"'
    )


def test_startup_loads_generated_secret_and_rejects_missing_secret(
    tmp_path, monkeypatch
):
    from agent.app import control_token

    monkeypatch.delenv("LAB_CONTROL_TOKEN_FILE", raising=False)
    monkeypatch.delenv("LAB_CONTROL_TOKEN", raising=False)
    with pytest.raises(RuntimeError, match="control credential"):
        control_token()
    token = "private" * 8
    path = tmp_path / "token"
    path.write_text(token + "\n")
    monkeypatch.setenv("LAB_CONTROL_TOKEN_FILE", str(path))
    assert control_token() == token
    engine = AsyncMock()
    engine.scenarios = lambda: []
    engine.health.return_value = {"tumult": True, "target": True, "version": "test"}
    constructor = Mock(return_value=engine)
    monkeypatch.setattr("agent.engine.LabEngine", constructor)
    monkeypatch.setenv("LAB_DATA_DIR", str(tmp_path))
    with TestClient(
        create_app(web_dir=tmp_path), base_url="http://localhost:8089"
    ) as client:
        assert client.get("/api/status").json()["engine"]["available"]
    assert constructor.call_args.kwargs["control_token"] == token
    engine.initialize.assert_awaited_once()
    engine.close.assert_awaited_once()


def test_session_capacity_fails_closed_and_existing_browser_still_works(
    lab, monkeypatch
):
    client, _, _ = lab
    monkeypatch.setattr("agent.app.MAX_SESSIONS", 1)
    session(client)
    cookie = client.cookies.get(COOKIE)
    client.cookies.clear()
    assert client.get("/api/status").status_code == 503
    client.cookies.set(COOKIE, cookie)
    assert client.get("/api/status").status_code == 200


@pytest.mark.parametrize(
    "size,expected", [("no-number", 400), ("-1", 413), ("99999", 413)]
)
def test_invalid_declared_size_is_rejected(lab, size, expected):
    client, _, _ = lab
    session(client)
    response = client.post(
        "/api/settings",
        content=b"{}",
        headers={"Content-Type": "application/json", "Content-Length": size},
    )
    assert response.status_code == expected


def test_non_json_payload_and_missing_cloud_key_are_explicit_errors(lab):
    client, _, _ = lab
    session(client)
    assert (
        client.post(
            "/api/settings", content=b"hello", headers={"Content-Type": "text/plain"}
        ).status_code
        == 415
    )
    assert (
        client.post(
            "/api/settings",
            json={"provider": "openai", "model": "gpt-4.1-mini", "api_key": ""},
        ).status_code
        == 400
    )
    client.post("/api/settings", json={"provider": "ollama", "model": "llama3.2:3b"})
    assert client.post("/api/ask", json={"q": "  "}).status_code == 400


def test_catalogue_history_stop_and_invalid_identifier(lab):
    client, engine, _ = lab
    session(client)
    assert client.get("/api/scenarios").json() == [
        {"id": "latency", "title": "Latency"}
    ]
    assert client.get("/api/runs").json() == []
    assert client.get("/api/runs/bad%20id").status_code == 404
    engine.get.side_effect = KeyError("missing")
    assert client.get("/api/runs/missing").status_code == 404
    engine.get.side_effect = None
    engine.get.return_value = {"id": "run-one"}
    engine.stop.return_value = {"id": "run-one", "status": "stopped"}
    assert client.post("/api/runs/run-one/stop", json={}).json()["status"] == "stopped"

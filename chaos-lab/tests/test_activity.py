"""Fixed activity boundaries: real evidence files, mocked network and parent APIs."""

import io
import json
import runpy
import signal
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock
from urllib.error import HTTPError, URLError

import pytest

from agent import lab_activity as activity


@pytest.fixture
def activity_context(tmp_path, monkeypatch):
    monkeypatch.setattr(activity, "bind_parent_lifetime", lambda: None)
    monkeypatch.setenv("LAB_RUN_DIR", str(tmp_path))
    monkeypatch.setenv("LAB_TARGET_URL", "http://target:5000")
    monkeypatch.setenv("LAB_CONTROL_TOKEN", "private-control-token")
    monkeypatch.setenv("LAB_FAULT", "latency")
    return tmp_path


@pytest.mark.parametrize(
    "mode,healthy,exit_code",
    [
        ("baseline", True, 0),
        ("baseline", False, 1),
        ("during", False, 0),
        ("recovery", False, 0),
    ],
)
def test_probe_persists_measured_evidence(
    activity_context, monkeypatch, capsys, mode, healthy, exit_code
):
    measurements = {"healthy": healthy, "requests": 8, "p95_ms": 401.0}
    monkeypatch.setattr(activity.sys, "argv", ["helper", mode])
    monkeypatch.setattr(activity, "sample", lambda target: measurements)
    assert activity.main() == exit_code
    assert json.loads(capsys.readouterr().out) == measurements
    assert json.loads((activity_context / (mode + ".json")).read_text()) == measurements
    assert json.loads((activity_context / "phase.json").read_text()) == {"phase": mode}


def test_repeated_steady_probe_preserves_original_snapshots(
    activity_context, monkeypatch
):
    monkeypatch.setattr(activity.sys, "argv", ["helper", "steady"])
    samples = iter([{"healthy": True}, {"healthy": False}])
    monkeypatch.setattr(activity, "sample", lambda target: next(samples))
    assert activity.main() == 0
    before = (activity_context / "baseline.json").read_bytes()
    (activity_context / "during.json").write_text(
        '{"healthy": false, "original": true}'
    )
    assert activity.main() == 0
    assert (activity_context / "baseline.json").read_bytes() == before
    assert json.loads((activity_context / "during.json").read_text())["original"]


@pytest.mark.parametrize(
    "mode,path,payload",
    [
        ("inject", "/fault", {"fault": "latency", "duration_s": 12}),
        ("reset", "/reset", {}),
    ],
)
def test_control_only_sends_fixed_payload(
    activity_context, monkeypatch, mode, path, payload
):
    request = Mock(return_value=(200, {"status": "acknowledged"}))
    monkeypatch.setattr(activity, "request", request)
    monkeypatch.setattr(activity.sys, "argv", ["helper", mode])
    assert activity.main() == 0
    request.assert_called_once_with(
        "http://target:5000", path, "private-control-token", payload
    )


def test_control_rejection_is_not_reported_as_success(activity_context, monkeypatch):
    monkeypatch.setattr(activity, "request", lambda *args: (401, {"error": "denied"}))
    monkeypatch.setattr(activity.sys, "argv", ["helper", "inject"])
    with pytest.raises(RuntimeError, match="HTTP 401"):
        activity.main()


@pytest.mark.parametrize(
    "arguments",
    [[], ["helper"], ["helper", "shell"], ["helper", "inject", "arbitrary"]],
)
def test_activity_rejects_unsupported_arguments(
    activity_context, monkeypatch, arguments
):
    monkeypatch.setattr(activity.sys, "argv", arguments)
    with pytest.raises(ValueError, match="Unknown activity"):
        activity.main()
    assert not list(activity_context.iterdir())


def test_sampling_counts_network_errors_and_failed_http(monkeypatch):
    replies = iter(
        [
            (200, {"dependencies": {"cache": "ok"}}),
            URLError("offline"),
            (503, {"dependencies": {"cache": "unavailable"}}),
        ]
    )

    def fetch(*args):
        result = next(replies)
        if isinstance(result, Exception):
            raise result
        return result

    monkeypatch.setattr(activity, "SAMPLE_COUNT", 3)
    monkeypatch.setattr(activity, "request", fetch)
    monkeypatch.setattr(activity.time, "sleep", lambda _: None)
    result = activity.sample("http://target:5000")
    assert result["requests"] == 3
    assert result["errors"] == 2
    assert result["error_rate"] == pytest.approx(2 / 3)
    assert result["healthy"] is False
    assert result["samples"][1]["status"] == 0
    assert result["samples"][1]["dependencies"] == {"error": "URLError"}


@pytest.mark.parametrize("status", [200, 503])
def test_http_boundary_parses_success_and_error_without_losing_status(
    monkeypatch, status
):
    body = b'{"status":"observed"}'

    class Response(io.BytesIO):
        pass

    response = Response(body)
    response.status = status

    def open_request(request, timeout):
        assert request.full_url == "http://target:5000/fault"
        assert request.get_header("Authorization") == "Bearer private"
        assert json.loads(request.data) == {"fault": "latency"}
        assert timeout == 2
        if status == 503:
            raise HTTPError(request.full_url, status, "Unavailable", {}, response)
        return response

    monkeypatch.setattr(activity.OPENER, "open", open_request)
    assert activity.request(
        "http://target:5000", "/fault", "private", {"fault": "latency"}
    ) == (status, {"status": "observed"})


@pytest.mark.parametrize(
    "platform,prctl_result,parents,error",
    [
        ("darwin", 0, [42], RuntimeError),
        ("linux", -1, [42], OSError),
        ("linux", 0, [1], RuntimeError),
        ("linux", 0, [42, 43], RuntimeError),
        ("linux", 0, [42, 42], None),
    ],
)
def test_parent_lifetime_fails_closed_without_changing_test_process(
    monkeypatch, platform, prctl_result, parents, error
):
    prctl = Mock(return_value=prctl_result)
    monkeypatch.setattr(activity.sys, "platform", platform)
    monkeypatch.setattr(
        activity.ctypes, "CDLL", lambda *args, **kwargs: SimpleNamespace(prctl=prctl)
    )
    monkeypatch.setattr(activity.ctypes, "get_errno", lambda: 1)
    parent_values = iter(parents)
    monkeypatch.setattr(activity.os, "getppid", lambda: next(parent_values))
    if error:
        with pytest.raises(error):
            activity.bind_parent_lifetime()
    else:
        activity.bind_parent_lifetime()
        prctl.assert_called_once_with(1, signal.SIGTERM, 0, 0, 0)


def test_helper_script_rejects_unknown_mode_before_network(monkeypatch):
    monkeypatch.setattr(activity.sys, "argv", ["helper", "unsupported"])
    monkeypatch.setattr(
        activity.ctypes,
        "CDLL",
        lambda *args, **kwargs: SimpleNamespace(prctl=lambda *args: 0),
    )
    monkeypatch.setattr(activity.os, "getppid", lambda: 42)
    with pytest.raises(ValueError, match="Unknown activity"):
        runpy.run_path(str(Path(activity.__file__)), run_name="__main__")

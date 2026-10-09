import importlib.util
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location(
    "lab_target", Path(__file__).parents[1] / "targets/api/app.py"
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def test_faults_expire_and_reset_interrupts_wait():
    clock = [10.0]
    state = module.FaultState(clock=lambda: clock[0])
    state.apply("latency", 5)
    assert state.current() == "latency"
    clock[0] = 16
    assert state.current() is None
    state.apply("db-timeout", 2)
    state.reset()
    assert state.current() is None


@pytest.mark.parametrize(
    "fault,duration",
    [
        ("shell", 5),
        ("latency", 0),
        ("latency", 16),
        ("latency", True),
        ("latency", float("nan")),
    ],
)
def test_fault_rejects_unbounded_values(fault, duration):
    with pytest.raises(ValueError):
        module.FaultState().apply(fault, duration)


def test_http_control_requires_token_and_work_measures_actual_fault():
    import json
    import threading
    from urllib.error import HTTPError
    from urllib.request import Request, urlopen

    token = "a-secure-test-token"
    server = module.make_server("127.0.0.1", 0, token)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base = f"http://127.0.0.1:{server.server_port}"
    try:
        with pytest.raises(HTTPError) as exc:
            urlopen(Request(base + "/fault", data=b"{}"), timeout=2)
        assert exc.value.code == 401
        payload = json.dumps({"fault": "cache-outage", "duration_s": 1}).encode()
        with urlopen(
            Request(
                base + "/fault",
                data=payload,
                headers={"Authorization": "Bearer " + token},
            ),
            timeout=2,
        ) as response:
            assert response.status == 200
        with pytest.raises(HTTPError) as exc:
            urlopen(base + "/work", timeout=2)
        assert exc.value.code == 503
        assert (
            json.loads(exc.value.read())["dependencies"]["cache"]
            == "simulated-unavailable"
        )
        with urlopen(
            Request(
                base + "/reset",
                data=b"{}",
                headers={"Authorization": "Bearer " + token},
            ),
            timeout=2,
        ):
            pass
        with urlopen(base + "/work", timeout=2) as response:
            assert response.status == 200
    finally:
        server.shutdown()
        server.server_close()
        thread.join()

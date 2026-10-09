"""Runnable container topology must preserve the local sandbox boundary."""

from pathlib import Path

import yaml

ROOT = Path(__file__).parents[1]


def compose():
    return yaml.safe_load((ROOT / "docker-compose.yml").read_text())


def test_only_browser_port_is_published_and_it_is_loopback():
    services = compose()["services"]
    published = [
        port for service in services.values() for port in service.get("ports", [])
    ]
    assert published == ["127.0.0.1:${LAB_PORT:-8089}:8000"]


def test_no_host_authority_and_unprivileged_runtime():
    services = compose()["services"]
    for name, service in services.items():
        assert not service.get("privileged")
        assert "ALL" in service["cap_drop"]
        assert "no-new-privileges:true" in service["security_opt"]
        assert service.get("network_mode") != "host"
        assert "docker.sock" not in str(service)
        if name != "setup":
            assert service["user"] == "10001:10001"
            assert service["read_only"] is True


def test_target_has_no_internet_and_requires_private_control_token():
    value = compose()
    assert value["networks"]["sandbox"]["internal"] is True
    target = value["services"]["target"]
    assert target["networks"] == ["sandbox"]
    assert target["environment"]["LAB_CONTROL_TOKEN_FILE"] == "/control/token"
    assert "control:/control:ro" in target["volumes"]


def test_keys_are_not_build_or_environment_requirements():
    data = (ROOT / "docker-compose.yml").read_text()
    assert "API_KEY" not in data
    assert "${TUMULT_MCP_TOKEN" not in data
    assert "latest" not in data


def test_build_installs_checksum_pinned_real_tumult():
    dockerfile = (ROOT / "agent/Dockerfile").read_text()
    assert "install_tumult.py" in dockerfile
    installer = (ROOT / "scripts/install_tumult.py").read_text()
    assert 'VERSION = "2.22.0"' in installer
    assert "sha256" in installer
    assert "extractall" not in installer

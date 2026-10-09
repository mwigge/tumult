#!/usr/bin/env python3
"""Smoke a host-compatible daemon artifact using private state and loopback HTTP.

The child receives an allowlisted environment, with no inherited HOME, proxy,
bootstrap or extension settings. This is not an OS network sandbox: the database
extension regression and network-disabled image smoke enforce offline operation.
"""

import argparse
import json
import os
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


def loopback_ports() -> tuple[int, int]:
    with socket.socket() as http, socket.socket() as grpc:
        http.bind(("127.0.0.1", 0))
        grpc.bind(("127.0.0.1", 0))
        return http.getsockname()[1], grpc.getsockname()[1]


def request(port: int, path: str, token: str | None) -> bytes:
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    query = urllib.request.Request(f"http://127.0.0.1:{port}{path}", headers=headers)
    # Never send local probe traffic through an inherited HTTP(S) proxy.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(query, timeout=3) as response:
        return response.read()


def wait_ready(
    process: subprocess.Popen[bytes], port: int, token: str, timeout: int
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(
                f"daemon exited before readiness (code {process.returncode})"
            )
        try:
            request(port, "/readyz", token)
            return
        except (OSError, urllib.error.URLError):
            time.sleep(0.1)
    raise TimeoutError("authenticated daemon readiness timed out")


def stop(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            raise RuntimeError("daemon required forced shutdown") from None
    if process.returncode != 0:
        raise RuntimeError(f"daemon exited with code {process.returncode}")


def phase(
    binary: Path, directory: Path, env: dict[str, str], token: str, timeout: int
) -> None:
    http_port, grpc_port = loopback_ports()
    env = dict(
        env,
        KRONIKA_OTLP_HTTP_ADDR=f"127.0.0.1:{http_port}",
        KRONIKA_OTLP_GRPC_ADDR=f"127.0.0.1:{grpc_port}",
    )
    # Child diagnostics may contain credentials. Keep them private and delete
    # them with the owned temporary directory; never print arbitrary child logs.
    with (directory / "daemon.log").open("ab") as log:
        process = subprocess.Popen(
            [str(binary), "serve"], cwd=directory, env=env, stdout=log, stderr=log
        )
        try:
            wait_ready(process, http_port, token, timeout)
            request(http_port, "/healthz", token)
            identity = json.loads(request(http_port, "/api/me", token))
            if not (
                identity.get("auth_required")
                and identity.get("authenticated")
                and identity.get("username") == "admin"
            ):
                raise RuntimeError(
                    "persisted administrator identity was not authenticated"
                )
            metrics = json.loads(request(http_port, "/api/metrics", token))
            if not any(
                metric.get("name") == "hypothesis_pass_rate"
                for metric in metrics.get("metrics", [])
            ):
                raise RuntimeError("semantic metric definitions are unavailable")
            try:
                request(http_port, "/healthz", None)
            except urllib.error.HTTPError as error:
                if error.code != 401:
                    raise RuntimeError(
                        "unauthenticated health returned an unexpected status"
                    ) from None
            else:
                raise RuntimeError("unauthenticated health request was accepted")
        finally:
            stop(process)


def smoke(binary: Path, metrics: Path, timeout: int) -> None:
    token = "kro_" + secrets.token_hex(32)
    with tempfile.TemporaryDirectory(prefix="tumult-binary-smoke-") as name:
        directory = Path(name)
        env = {
            "PATH": os.defpath,
            "TMPDIR": str(directory),
            "RUST_LOG": "warn",
            "TUMULT_LAKE_PATH": str(directory / "lake.duckdb"),
            "KRONIKA_METRICS_DIR": str(metrics),
            "KRONIKA_INGEST_TOKEN": "kro_" + secrets.token_hex(32),
            "KRONIKA_RETENTION_DAYS": "0",
            "TUMULTD_RUN_RETENTION_DAYS": "0",
        }
        bootstrap = dict(
            env,
            KRONIKA_BOOTSTRAP_ADMIN_PASSWORD=secrets.token_hex(32),
            KRONIKA_BOOTSTRAP_TOKEN=token,
        )
        phase(binary, directory, bootstrap, token, timeout)
        phase(binary, directory, env, token, timeout)
    print(
        "daemon binary smoke passed: health, metrics, authentication and persisted identity"
    )


def interrupted(_signum: int, _frame: object) -> None:
    raise KeyboardInterrupt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument(
        "--metrics-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "metrics",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=90,
        help="readiness timeout per startup, 1–300 seconds",
    )
    args = parser.parse_args()
    if not 1 <= args.timeout <= 300:
        parser.error("--timeout must be between 1 and 300")
    signal.signal(signal.SIGTERM, interrupted)
    try:
        smoke(
            args.binary.resolve(strict=True),
            args.metrics_dir.resolve(strict=True),
            args.timeout,
        )
    except KeyboardInterrupt:
        return 130
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"daemon binary smoke failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

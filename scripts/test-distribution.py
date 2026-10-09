#!/usr/bin/env python3
"""Regression checks for deployment contracts and installer failure handling."""

import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from threading import Thread

ROOT = Path(__file__).resolve().parents[1]


class DistributionTests(unittest.TestCase):
    def test_container_and_kubernetes_arguments_compose(self):
        image = (ROOT / "docker/Dockerfile.tumult-mcp").read_text()
        manifest = (ROOT / "deploy/k8s/tumult-mcp.yaml").read_text()
        entrypoint = json.loads(re.search(r"^ENTRYPOINT (.+)$", image, re.M)[1])
        args = json.loads(re.search(r"^\s+args: (.+)$", manifest, re.M)[1])
        command = re.search(r"^\s+command: (.+)$", manifest, re.M)
        argv = (json.loads(command[1]) if command else entrypoint) + args
        flags = [arg for arg in argv if arg.startswith("--")]
        self.assertEqual(len(flags), len(set(flags)), argv)
        self.assertEqual(argv[0], "tumult-mcp")
        self.assertEqual(argv[argv.index("--host") + 1], "0.0.0.0")

    def test_service_workspaces_are_persisted_and_writable(self):
        manifest = (ROOT / "deploy/k8s/tumult-mcp.yaml").read_text()
        workdir = re.search(r"^\s+workingDir: (.+)$", manifest, re.M)
        self.assertIsNotNone(workdir, "MCP writes journals in its working directory")
        mounts = re.findall(r"mountPath: ([^ }]+)", manifest)
        self.assertIn(workdir[1], mounts)
        self.assertIn("/tmp", mounts)
        unit = (ROOT / "deploy/systemd/tumult-mcp.service").read_text()
        state = re.search(r"^StateDirectory=(.+)$", unit, re.M)[1]
        self.assertIn(f"WorkingDirectory=/var/lib/{state}", unit)

    def test_daemon_packages_script_plugins_and_network_helper(self):
        image = (ROOT / "docker/Dockerfile.tumultd").read_text()
        self.assertRegex(image, r"cargo build[^\n]+-p tumult-net")
        self.assertRegex(
            image, r"COPY --from=builder .*/tumult-net-proxyd /usr/local/bin/"
        )
        self.assertRegex(image, r"COPY plugins/ /opt/tumult/plugins/")
        self.assertIn("TUMULT_PLUGIN_PATH=/opt/tumult/plugins", image)

    def test_daemon_templates_disable_automatic_retention(self):
        manifest = (ROOT / "deploy/k8s/tumultd.yaml").read_text()
        unit = (ROOT / "deploy/systemd/tumultd.service").read_text()
        self.assertRegex(manifest, r'name: TUMULTD_RUN_RETENTION_DAYS, value: "0"')
        self.assertIn("Environment=TUMULTD_RUN_RETENTION_DAYS=0\n", unit)

    def test_daemon_uses_packaged_metrics(self):
        manifest = (ROOT / "deploy/k8s/tumultd.yaml").read_text()
        path = re.search(r"name: KRONIKA_METRICS_DIR\s+value: (.+)", manifest)[1]
        image = (ROOT / "docker/Dockerfile.tumultd").read_text()
        self.assertIn(f"COPY metrics/ {path}/", image)

    def test_daemon_workspaces_and_temporary_files_are_writable(self):
        manifest = (ROOT / "deploy/k8s/tumultd.yaml").read_text()
        self.assertIn("workingDir: /data", manifest)
        self.assertIn("runAsUser: 10001", manifest)
        self.assertIn("fsGroup: 10001", manifest)
        mounts = re.findall(r"mountPath: ([^ }]+)", manifest)
        self.assertIn("/data", mounts)
        self.assertIn("/tmp", mounts)
        self.assertRegex(manifest, r"emptyDir: \{ sizeLimit: [^}]+\}")
        unit = (ROOT / "deploy/systemd/tumultd.service").read_text()
        state = re.search(r"^StateDirectory=(.+)$", unit, re.M)[1]
        self.assertIn(f"WorkingDirectory=/var/lib/{state}", unit)

    def test_daemon_probe_credential_can_rotate_separately_from_bootstrap(self):
        manifest = (ROOT / "deploy/k8s/tumultd.yaml").read_text()
        secrets = {}
        for name in ("KRONIKA_BOOTSTRAP_TOKEN", "TUMULTD_PROBE_TOKEN"):
            ref = re.search(
                rf"name: {name}\s+valueFrom:\s+secretKeyRef: \{{ name: ([^,]+), key: ([^ }}]+) \}}",
                manifest,
            )
            self.assertIsNotNone(ref, f"{name} must come from a required Secret")
            secrets[name] = ref.groups()
        self.assertNotEqual(*secrets.values())

    @unittest.skipUnless(shutil.which("curl"), "curl required for real probe execution")
    def test_daemon_probes_authenticate_and_fail_closed(self):
        manifest = (ROOT / "deploy/k8s/tumultd.yaml").read_text()
        commands = []
        for name, endpoint in (
            ("livenessProbe", "/healthz"),
            ("readinessProbe", "/readyz"),
        ):
            probe = manifest.split(f"          {name}:\n", 1)[1]
            probe = probe.split("            initialDelaySeconds:", 1)[0]
            match = re.search(r"command: (\[.+\])", probe)
            self.assertIsNotNone(match, "authenticated probes must use exec")
            command = json.loads(match[1])
            self.assertIn(endpoint, command[-1])
            commands.append(command)
        requests = []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                token = self.headers.get("Authorization")
                requests.append((self.path, token))
                self.send_response(
                    200 if token == "Bearer synthetic-valid-token" else 401
                )
                self.end_headers()

            def log_message(self, *_args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            for command in commands:
                command[-1] = command[-1].replace(":4318/", f":{server.server_port}/")
                for token, expected in (
                    ("synthetic-valid-token", 0),
                    ("invalid", 22),
                    ("", 1),
                ):
                    with self.subTest(probe=command[-1], token=token):
                        before = len(requests)
                        result = subprocess.run(
                            command,
                            env={**os.environ, "TUMULTD_PROBE_TOKEN": token},
                            capture_output=True,
                            text=True,
                            timeout=5,
                        )
                        self.assertEqual(result.returncode, expected, result.stderr)
                        self.assertEqual(len(requests) - before, 1 if token else 0)
                        if token:
                            self.assertEqual(requests[-1][1], f"Bearer {token}")
        finally:
            server.shutdown()
            thread.join(timeout=5)
            server.server_close()

    def test_release_archives_include_the_native_network_helper(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        for command in ("cargo build --release", "cross build --release"):
            line = next(line for line in release.splitlines() if command in line)
            self.assertIn("-p tumult-net", line)
        for archive in ("ARCHIVE", "ARCHIVE_D"):
            self.assertRegex(
                release,
                rf'cp [^\n]*release/tumult-net-proxyd[^\n]*"\$\{{{archive}\}}/"',
            )

    def test_release_publishes_the_pinned_daemon_image(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("file: docker/Dockerfile.tumultd", release)
        self.assertIn("/tumultd:${{ steps.version.outputs.version }}", release)
        version = re.search(
            r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M
        )[1]
        manifest = (ROOT / "deploy/k8s/tumultd.yaml").read_text()
        self.assertIn(f"image: ghcr.io/mwigge/tumultd:{version}\n", manifest)

    def test_authenticated_demo_healthcheck_can_reach_the_api(self):
        compose = (ROOT / "docker/docker-compose.kronika.yml").read_text()
        healthcheck = compose.split("    healthcheck:", 1)[1].split("    restart:", 1)[
            0
        ]
        self.assertIn("Authorization: Bearer", healthcheck)
        self.assertIn("$${KRONIKA_BOOTSTRAP_TOKEN}", healthcheck)

    def test_demo_with_seeded_credentials_only_publishes_loopback(self):
        compose = (ROOT / "docker/docker-compose.kronika.yml").read_text()
        ports = compose.split("    ports:", 1)[1].split("    environment:", 1)[0]
        published = re.findall(r'^\s+- "([^"]+)"$', ports, re.M)
        self.assertTrue(published)
        self.assertTrue(all(port.startswith("127.0.0.1:") for port in published))

    def run_installer(self, failure):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text("[workspace]\n# tumult\n")
            binary = root / "target/release/tumult"
            binary.parent.mkdir(parents=True)
            binary.write_text('#!/bin/sh\necho stale-binary-executed >> "$MOCK_LOG"\n')
            binary.chmod(0o755)
            mock_bin = root / "bin"
            mock_bin.mkdir()
            for command in ("git", "cargo", "docker", "make", "cp", "sleep"):
                mock = mock_bin / command
                mock.write_text(
                    "#!/bin/sh\n"
                    f'echo "{command} $*" >> "$MOCK_LOG"\n'
                    f'[ "$MOCK_FAILURE" != "{command}" ] || exit 23\n'
                    "exit 0\n"
                )
                mock.chmod(0o755)
            log = root / "calls.log"
            result = subprocess.run(
                ["sh", str(ROOT / "install.sh")],
                cwd=root,
                env={
                    **os.environ,
                    "PATH": f"{mock_bin}:{os.environ['PATH']}",
                    "MOCK_FAILURE": failure,
                    "MOCK_LOG": str(log),
                },
                capture_output=True,
                text=True,
                timeout=10,
            )
            return result, log.read_text()

    def test_failed_build_never_installs_or_verifies_stale_binary(self):
        result, calls = self.run_installer("cargo")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("cp ", calls)
        self.assertNotIn("stale-binary-executed", calls)

    def test_failed_target_start_never_claims_readiness(self):
        result, calls = self.run_installer("make")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Docker targets started", result.stdout)
        self.assertNotIn("stale-binary-executed", calls)


class JournalProofTests(unittest.TestCase):
    def check_journal(self, content):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "journal.toon"
            path.write_text(content)
            return subprocess.run(
                ["python3", str(ROOT / "scripts/check-proof-journals.py"), str(path)],
                capture_output=True,
                text=True,
                timeout=5,
            )

    def test_completed_experiment_is_accepted(self):
        self.assertEqual(
            self.check_journal("status: completed\nrollback_failures: 0\n").returncode,
            0,
        )

    def test_completed_word_in_failed_output_is_rejected(self):
        result = self.check_journal(
            'status: deviated\noutput: "completed"\nrollback_failures: 0\n'
        )
        self.assertNotEqual(result.returncode, 0)

    def test_all_gameday_experiments_must_complete(self):
        content = (
            "gameday_id: test\nexperiment_journals[2]:\n"
            "  - experiment_title: first\n    status: completed\n    rollback_failures: 0\n"
            "  - experiment_title: second\n    status: deviated\n    rollback_failures: 0\n"
        )
        self.assertNotEqual(self.check_journal(content).returncode, 0)
        self.assertEqual(
            self.check_journal(content.replace("deviated", "completed")).returncode, 0
        )

    def test_missing_gameday_record_is_rejected(self):
        content = (
            "gameday_id: test\nexperiment_journals[2]:\n"
            "  - experiment_title: first\n    status: completed\n    rollback_failures: 0\n"
        )
        self.assertNotEqual(self.check_journal(content).returncode, 0)

    def test_rollback_failure_is_not_success(self):
        self.assertNotEqual(
            self.check_journal("status: completed\nrollback_failures: 1\n").returncode,
            0,
        )


if __name__ == "__main__":
    unittest.main()

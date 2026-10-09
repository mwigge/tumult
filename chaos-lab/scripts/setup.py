"""Provision the sandbox credential automatically; API keys are never stored here."""

import os
import secrets
from pathlib import Path


def main() -> None:
    directory = Path("/control")
    token = directory / "token"
    if not token.exists():
        descriptor = os.open(token, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "w") as handle:
            handle.write(secrets.token_urlsafe(48))
    os.chown(token, 10001, 10001)
    os.chmod(token, 0o600)
    # Keep the directory owned by setup so a subsequent setup can validate ownership.
    os.chown(directory, 0, 10001)
    # Shared control directory permits only the runtime group to traverse it.
    os.chmod(directory, 0o750)  # nosec B103


if __name__ == "__main__":
    main()

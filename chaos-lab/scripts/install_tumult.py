"""Install the verified, architecture-specific Tumult release during image build."""

import hashlib
import io
import platform
import tarfile
import urllib.request
from pathlib import Path

VERSION = "2.22.0"
ASSETS = {
    "x86_64": (
        "x86_64",
        "31f92395129aecb097e0c7d06fcfe8134dc399400b7e09ab9cf9eac21075c1a5",
    ),
    "aarch64": (
        "aarch64",
        "5956ceff360b9f0e9c58695348a67a66be848e91785657b2ea4fb6172c55552b",
    ),
}


def main() -> None:
    """Only verified executable members are copied; paths are never extracted."""
    machine = platform.machine()
    if machine not in ASSETS:
        raise RuntimeError("This lab supports Linux amd64 and arm64 containers.")
    architecture, checksum = ASSETS[machine]
    name = f"tumult-v{VERSION}-{architecture}-unknown-linux-musl"
    url = f"https://github.com/mwigge/tumult/releases/download/v{VERSION}/{name}.tar.gz"
    # URL components above are fixed release constants; SHA256 verifies the body.
    with urllib.request.urlopen(url, timeout=120) as response:  # nosec B310
        data = response.read(100_000_001)
    if len(data) > 100_000_000 or hashlib.sha256(data).hexdigest() != checksum:
        raise RuntimeError("Tumult release checksum verification failed.")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for binary in ("tumult", "tumult-net-proxyd"):
            member = archive.getmember(f"{name}/{binary}")
            if not member.isfile() or member.size > 150_000_000:
                raise RuntimeError("Unexpected Tumult archive member.")
            source = archive.extractfile(member)
            if source is None:
                raise RuntimeError("Missing Tumult executable.")
            target = Path("/usr/local/bin") / binary
            target.write_bytes(source.read())
            target.chmod(0o755)


if __name__ == "__main__":
    main()

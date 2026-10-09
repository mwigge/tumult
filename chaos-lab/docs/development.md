# Develop and test the lab

End users do not need Python or Node installed on the host. For development on Linux, install Python 3.12 or newer and Node 22.17 or newer. Start the lab first using the [quick start](../README.md#1-start-the-lab), then run these commands from `chaos-lab/`:

```sh
python3 -m venv .venv
.venv/bin/pip install -r requirements-dev.txt
npm ci
npx playwright install chromium
mkdir -p .lab
docker compose cp lab:/usr/local/bin/tumult .lab/tumult
export TUMULT_TEST_BIN="$PWD/.lab/tumult"
./scripts/check.sh
```

The full check requires the real binary; it does not silently skip those integration tests. The browser contract uses local fixture responses, while these commands exercise the running Compose lab without AI credentials:

```sh
.venv/bin/python tests/browser_live.py
.venv/bin/python tests/restart_live.py
```

The second check deliberately kills only this lab's application container during a bounded fault, verifies autonomous expiry, and restarts the application. It preserves evidence volumes. Generated screenshots, downloads, and verification records are placed in `test-results/`.

`./seed.sh` optionally runs all four guided lessons to populate the local history. It uses the fixed experiment API and requires no model or API key.

[Back to the lab setup](../README.md).

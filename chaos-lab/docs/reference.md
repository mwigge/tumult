# Lab behavior and data

| Lesson            | Actual change in the teaching API                             | What to compare                                                |
| ----------------- | ------------------------------------------------------------- | -------------------------------------------------------------- |
| Slow responses    | Adds 400 ms before returning a response                       | A successful HTTP status can still violate a latency objective |
| HTTP failures     | Returns HTTP 503 during the fault window                      | Low latency does not imply availability                        |
| Unavailable cache | Simulates an unavailable cache dependency and returns failure | Dependency coupling and graceful degradation                   |
| Database timeout  | Waits 600 ms and returns HTTP 503                             | Timeout budgets and combined latency / availability impact     |

These are **application-level dependency simulations with real HTTP responses and timings**. There is no real Redis or PostgreSQL server to break. The lab does not inject kernel packet loss, stress the host, stop arbitrary containers, or contact a user-selected fault target. It uses a fixed catalogue executed by the real Tumult binary.

A result of **deviated** is an expected learning outcome: the objective failed during the injected fault and recovered afterward. **Inconclusive** means the evidence does not support the intended comparison, for example because baseline or recovery failed or the run was stopped. The native Tumult journal can report successful completion and recovery while the lab reports a fault-window deviation; they answer different questions. See [interpreting results](learning-guide.md#interpret-the-two-results).

## Data, privacy, and the local boundary

- The only published port is `127.0.0.1:8089` by default. The target has no published host port and runs on an internal Docker network.
- This is a **shared local workstation lab**, not a multiuser SaaS. There are no accounts, RBAC, or tenant boundaries. Anyone who can access this local lab can view its history and control the sandbox.
- AI credentials are scoped to a browser session and held in server memory. They are absent from browser local/session storage, evidence files, and exports. The browser session cookie expires up to one hour after creation. Server sessions also have a one-hour idle timeout and expired entries are removed during subsequent requests. Closing the browser alone does not promise immediate removal. **Disconnect** clears a connection, and restarting the lab clears all in-memory connections.
- Experiment records, measured samples, generated definitions, and native journals persist in the `evidence` Docker volume. The generated target-control secret persists in a separate `control` volume. It is not an AI-provider credential.
- History is limited to 200 saved runs. Export evidence before intentionally resetting storage when that limit is reached.
- Cloud tutors receive the question and run context when you press **Ask tutor**. Guided experiments need no model traffic. Live paid-provider inference has not been validated with customer credentials.

For each completed run, use **Export evidence** for JSON, **Experiment definition** for the generated `.toon` definition, and **Tumult journal** for native execution evidence when available. Download files before resetting storage. The native format is described in the [Tumult experiment-format guide](https://github.com/mwigge/tumult/blob/v2.22.0/docs/guides/experiment-format.md).

## Runtime configuration

The lab runs two services: `lab` serves the browser/API and executes Tumult; `target` is the teaching API. A one-shot `setup` service generates their private control secret.

| Setting           | Default                             | Purpose                           |
| ----------------- | ----------------------------------- | --------------------------------- |
| `LAB_PORT`        | `8089`                              | Browser port on localhost         |
| `OLLAMA_BASE_URL` | `http://host.docker.internal:11434` | Operator-selected Ollama endpoint |

Set optional overrides in `chaos-lab/.env`, then run `docker compose up -d --wait` from `chaos-lab/`. Enter AI keys in the browser, not in `.env`.

The lab downloads the checksum-pinned Tumult 2.22.0 release; it does not compile the repository’s current Rust source. Update and verify the release hashes in `scripts/install_tumult.py` when changing that version.

All checkouts use the Compose project name `tumult-chaos-lab`. Starting the repo copy reuses an existing lab’s evidence volumes on the same Docker host. Run one copy at a time; use the repo copy for subsequent start/stop commands.

## Erase lab data

Export anything you want to keep first. From `chaos-lab/`, the following command removes **all saved runs and the generated control secret**:

```sh
docker compose down -v
```

Normal `docker compose down` keeps your saved runs.

[Back to the lab setup](../README.md).

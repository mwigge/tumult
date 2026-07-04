# Tumult 2.2 — One-command demo

A single, self-contained chaos-engineering showcase. One command stands up a
small but real system — an OTel-instrumented order service backed by Postgres,
under continuous load — plus the full Tumult platform (MCP chaos engine, OTel
collector, SigNoz) and a web control panel. It then injects one fault per
domain so the dashboards are populated the moment you open them.

Everything runs on a single Docker network, `tumult-demo`. The interface
contract (names, ports, env) is pinned in [`CONTRACT.md`](./CONTRACT.md).

## Quickstart

```bash
make demo          # build + up, import dashboards, run the fault sweep, print URLs
# ... explore, then:
make demo-down     # tear down and remove volumes
```

`make demo` builds the `tumult` base image (the MCP server is built from it),
brings the stack up, waits for health, runs the seven-domain fault sweep once
to warm the dashboards, imports the SigNoz dashboards, and prints the URLs.

Open the **control panel at http://localhost:8088** and click **Run** on any
fault card, then follow the **View traces** link into SigNoz.

## Architecture

```
                                  host ports
  ┌──────────────────────────────────────────────────────────────────────┐
  │  docker network: tumult-demo                                          │
  │                                                                        │
  │   demo-traffic ──HTTP──▶ demo-app ──SQL──▶ demo-postgres              │
  │   (baseline load)        :8080             :5433                       │
  │                            │                                           │
  │                            │ OTLP/http :4318                           │
  │                            ▼                                           │
  │                     tumult-collector ──────▶ signoz                    │
  │                     :14317/:14318            :3301  (traces/metrics)   │
  │                            ▲                                           │
  │                            │ OTLP/grpc :4317 (experiment run spans)    │
  │                            │                                           │
  │   demo-control-panel ──MCP──▶ tumult-mcp ──┬─ docker.sock ─▶ faults    │
  │   :8088  (web UI)      http  :3100         ├─ native net proxy         │
  │                                            ├─ native ssh ─▶ demo-sshd  │
  │                                            └─ reads /demo/experiments  │
  │                                                              :2222     │
  └──────────────────────────────────────────────────────────────────────┘
```

- **demo-app** emits a span per request; **demo-traffic** keeps a steady
  baseline so there is always live signal to disrupt.
- **tumult-mcp** is the chaos engine. The control panel is an MCP client of it;
  `make demo-check` drives the same `tumult run` path directly. It has the
  Docker socket mounted so container-scoped faults can reach sibling
  containers, and it ships the compiled native plugins (net, ssh).
- **tumult-collector** receives spans from both the app and the experiment runs
  and forwards them to **SigNoz**.

## Fault domains

One experiment per domain under [`experiments/`](./experiments/). Each has a
steady-state hypothesis, injects a real fault against the demo stack, and
verifies recovery — so a passing run is a resilience result, not just a
smoke-test tick.

| Domain     | Experiment                | Plugin / action (real)                          | What it does |
|------------|---------------------------|-------------------------------------------------|--------------|
| net        | `demo-net.toon`           | `tumult-net` native `inject_latency` / `stop_proxy` | Userspace tokio-netem proxy in front of demo-app; probes succeed through the +300ms delayed path |
| postgres   | `demo-postgres.toon`      | `docker exec demo-postgres psql` → `pg_terminate_backend` | Kills every active backend, verifies the DB keeps accepting connections |
| container  | `demo-container.toon`     | `docker pause` / `docker unpause demo-postgres` | Freezes the DB container briefly, unpauses, verifies recovery |
| stress     | `demo-stress.toon`        | Pumba `stress` (stress-ng sidecar) on demo-app  | Real CPU pressure inside demo-app's namespaces; app stays healthy |
| process    | `demo-process.toon`       | `docker kill -s SIGSTOP/SIGCONT demo-app`       | Suspends then resumes the app's main process, verifies recovery |
| ssh        | `demo-ssh.toon`           | `tumult-ssh` native `execute` → demo-sshd       | Runs commands (incl. a stress-ng burst) over SSH on the sshd target |
| agentic    | `demo-agentic.toon`       | `tumult agentic smoke` (bundled fake HTTP adapter) | Injects malformed model output and asserts the contract-feedback loop fires — no external API |
| agentic-trajectory | `demo-agentic-trajectory.toon` | `tumult agentic trajectory` (bundled fake adapters) | Multi-turn agent-graph: poisons retrieval at step 0, proves the answer step loses grounding, and detects a reflection loop — whole-trajectory contracts + agentic subscores, no external API |

Validate them yourself:

```bash
cargo build -p tumult-cli --bin tumult
for f in demo/experiments/*.toon; do ./target/debug/tumult validate "$f"; done
```

## Using the control panel

http://localhost:8088 shows one card per domain. Each card has a name, a
description, and a **Run** button that calls the MCP `run_experiment` tool for
that domain's experiment. Destructive actions are badged (a confirm step) using
the tool's MCP annotations. Live status shows running / passed / failed and the
run duration, and **View traces** deep-links into SigNoz filtered to
`service = demo-app` around the run window. The MCP bearer token comes from the
`TUMULT_MCP_TOKEN` env var (`tumult-demo` by default) — no build-time secrets.

## `make demo-check` — the smoke test

`make demo-check` is the headless functional smoke test used in development:

```bash
make demo-check    # up, wait for health, run all 7 experiments, assert telemetry, exit code
```

It brings the stack up, waits for every service to report healthy, runs each of
the seven experiments via `docker compose exec tumult-mcp tumult run …`, and
asserts:

1. **Each experiment ends `Completed`** — `tumult run` exits non-zero on any
   other status, so the fault injected *and* the system recovered.
2. **Telemetry landed** — it scrapes the collector's self-telemetry on
   `:18888` and asserts `otelcol_receiver_accepted_spans > 0` (falling back to
   grepping the collector logs for exported spans).

It prints `PASS`/`FAIL` per domain and a final summary, and **exits non-zero on
any failure**. The same script backs `make demo` in `--mode populate`, where it
runs the sweep best-effort to warm the dashboards without failing the caller.

## Ports (host)

| Port  | Service            |
|-------|--------------------|
| 8080  | demo-app           |
| 8088  | demo-control-panel |
| 5433  | demo-postgres      |
| 2222  | demo-sshd          |
| 3100  | tumult-mcp (HTTP)  |
| 3301  | SigNoz UI          |
| 14317 | collector OTLP gRPC |
| 14318 | collector OTLP HTTP |
| 18888 | collector self-telemetry (span counters) |
| 18889 | collector Prometheus (host + APM metrics) |
| 13133 | collector health   |

## Caveats

This demo runs in pure `docker compose` with no extra privilege beyond the
Docker socket. A few domains are scoped to what actually works in that
topology:

- **Kubernetes is intentionally excluded.** The demo is Docker-only; there is
  no k8s target. The `tumult-kubernetes` native plugin is still compiled into
  the MCP image and usable against a real cluster outside the demo.
- **net** — the tumult-net proxy sits **in front of demo-app** (probed via
  curl), not transparently between demo-app and demo-postgres. The app's DB URL
  is fixed at container start and cannot be rewired at runtime to route through
  the proxy, so we demonstrate the userspace latency proxy on a path we *can*
  route through. The latency injection itself is real.
- **postgres** — the runner image ships no `psql`, so connection-kill runs
  `docker exec demo-postgres psql …` (psql lives in the Postgres image) rather
  than the `tumult-db-postgres` script plugin's own client. Same SQL, same
  effect.
- **stress** — the `tumult-stress` plugin runs stress-ng in-process and cannot
  target another container. To genuinely pressure **demo-app** we use Pumba's
  `stress`, which launches a stress-ng sidecar sharing demo-app's cgroup/
  namespaces. First run pulls `ghcr.io/alexei-led/pumba` and its stress image.
- **process** — suspend/resume targets the app's main process via
  `docker kill -s SIGSTOP/SIGCONT` (signal to PID 1), which needs nothing
  installed inside the app container.
- **ssh** — `demo-sshd` copies its build-time private key onto a volume shared
  read-only with `tumult-mcp` so the native SSH executor can authenticate;
  `host_key_policy: accept-any` is used because the host key is ephemeral.
- **agentic** — the domain is exercised as a regular experiment whose method
  invokes `tumult agentic smoke` (the bundled fake-HTTP adapter), so it
  validates and runs like the others and needs **no external API**.

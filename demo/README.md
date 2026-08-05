# Tumult 2.2 — One-command demo

A single, self-contained chaos-engineering showcase. One command stands up a
small but real system — an OTel-instrumented order service backed by Postgres,
under continuous load — plus the full Tumult platform (MCP chaos engine, OTel
collector, SigNoz) and a web control panel. It then injects one fault per
domain so the dashboards are populated the moment you open them.

Everything runs on a single Docker network, `tumult-demo`, and every action in
the control panel is a single MCP `tools/call` against `tumult-mcp`. The
interface contract (names, ports, env) is pinned in [`CONTRACT.md`](./CONTRACT.md).

## The golden path

The demo tells one cohesive story on one stack. From the control panel at
**http://localhost:8088** you walk a single path — each step is an existing MCP
tool:

1. **Inject a fault** — click **Run** on any fault card (`tumult_run_experiment`).
2. **Observe** — follow the trace link into SigNoz, filtered to `service = demo-app`.
3. **Analyze** — the **Analytics** card queries recent runs (`tumult_analyze_store`).
4. **Check compliance** — the **Compliance** card renders the DORA evidence
   pass rate, verdict, citations and scope disclaimer (`tumult_compliance`).
5. **Explore the ChaosGraph** — the **ChaosGraph** card shows fault nodes, an
   experiment's ego sub-graph and coverage gaps (`tumult_chaosgraph_query` /
   `_neighbors` / `_coverage_gaps`).

A **Safety guardrail** card and a **full chaos loop** timeline
(`discover → validate → run → analyze → recommend`) round out the same path.

## Quickstart

```bash
make demo          # build + up, import dashboards, run the fault sweep, print URLs
# ... explore, then:
make demo-down     # tear down and remove volumes
```

`make demo` builds the `tumult` base image (the MCP server is built from it),
brings the stack up, waits for health, runs the fault sweep once to warm the
dashboards (and populate the analytics store the payoff cards read), imports
the SigNoz dashboards, and prints the URLs.

Open the **control panel at http://localhost:8088** and walk the golden path
above — start by clicking **Run** on any fault card.

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
| timewarp-clock | `demo-timewarp-clock.toon` | `tumult-timewarp` clock-skew (`token-ttl.sh` / `restore-clock.sh`) | Advances a validator's clock past a short-TTL HMAC token's expiry, proves the once-valid token is rejected, and confirms demo-app stays healthy |
| timewarp-entropy | `demo-timewarp-entropy.toon` | `tumult-timewarp` RNG pressure (`rng-pressure.sh` / `crypto-throughput.sh`) | Applies sustained RNG/crypto pressure on the runner and proves crypto operations still complete and entropy stays readable |

A safety-guardrail experiment, `demo-guard-halt.toon`, is **not** in the sweep
above: it arms an auto-halt guard so its expected outcome is `Halted`, not
`Completed`. It runs from the control panel's **Safety guardrail** card.

The topology/compliance pair — `demo-topo-blind.toon` and
`demo-topo-recommended.toon` — is likewise outside the sweep: both validate
demo-postgres recovery to close the NIS2 BC/DR gap flagged by the
recommender, and they belong to the topology proof flow in
[`scripts/demo-topology.sh`](../scripts/demo-topology.sh) (the blind variant
also serves as the autopilot playbook in
[`demo/topology/autopilot-blind.toml`](./topology/autopilot-blind.toml)).

Validate them yourself:

```bash
cargo build -p tumult-cli --bin tumult
for f in demo/experiments/*.toon; do ./target/debug/tumult validate "$f"; done
```

## Using the control panel

http://localhost:8088 is organised as the golden path, top to bottom:

- **Fault domains** — one card per domain with a **Run** button that calls the
  MCP `tumult_run_experiment` tool for that domain's experiment. Destructive
  actions are badged (a confirm step) using the tool's MCP annotations. Live
  status shows running / passed / failed and the run duration, and the trace
  link opens SigNoz with an explicit `filter service = demo-app` instruction
  (the traces-explorer page; SigNoz pre-filter query params vary by version).
- **Safety guardrail** — runs `demo-guard-halt.toon`. The auto-halt guard pulls
  the run the moment demo-app turns unhealthy and rollback restores Postgres, so
  the expected, badged outcome is **Halted** (not Completed).
- **The enterprise payoff** — three read-only cards over the persistent
  analytics store:
  - **Compliance** — `tumult_compliance {framework: dora, journals_path: /journals}`:
    the evidence pass rate, recovery proxy, verdict, sourced control citations,
    and the tool's own scope disclaimer (evidence toward controls, not an
    attestation).
  - **Analytics** — `tumult_analyze_store`: the most recent experiments
    (title / status / duration) as a compact table.
  - **ChaosGraph** — `tumult_chaosgraph_query {kind: fault}`, then
    `tumult_chaosgraph_neighbors` on a chosen experiment (its ego sub-graph),
    plus `tumult_chaosgraph_coverage_gaps {framework: dora}` (untested actions
    and unevidenced framework articles).
- **Run the whole chaos loop** — a timeline that drives
  `discover → validate → run → analyze → recommend` as five separate MCP calls.

The MCP bearer token comes from the `TUMULT_MCP_TOKEN` env var (`tumult-demo`
by default) — no build-time secrets. The Compliance corpus directory and
framework are configurable via `DEMO_JOURNALS_DIR` (default `/journals`) and
`DEMO_COMPLIANCE_FRAMEWORK` (default `dora`).

### `scripts/gameday-demo.sh` — the advanced / CI path

`scripts/gameday-demo.sh` is a scripted, non-interactive GameDay walkthrough
(discover → GameDay run → resilience analysis → analytics → compliance mapping)
intended for CI and terminal demos. It exercises the same MCP surface without
the web UI. Prefer `make demo` and the control panel for the interactive story;
reach for `gameday-demo.sh` when you want a headless, log-driven run.

## `make demo-check` — the smoke test

`make demo-check` is the headless functional smoke test used in development:

```bash
make demo-check    # up, wait for health, run the fault sweep, assert telemetry, exit code
```

It brings the stack up, waits for every service to report healthy, runs each
experiment in the sweep via `docker compose exec tumult-mcp tumult run …`, and
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

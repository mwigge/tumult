---
title: Production Deployment
parent: Guides
nav_order: 20
---

# Production Deployment

The demo (`make demo`) is a self-contained showcase. This guide covers running
the Tumult MCP server as a real service — securely, observably, and with a
recoverable store. The single-node experiment CLI (`tumult run/analyze/compliance`)
needs none of this; it applies when you expose the **MCP server** to agents or
operators over the network.

## 1. Security — read this first

The MCP server exposes tools that inject faults and can kill containers. Treat it
like any control-plane API.

- **Authentication is mandatory for network exposure.** The server **refuses to
  serve HTTP on a non-loopback address without configured auth**, and binds
  `127.0.0.1` by default — you must opt into a wider bind *and* configure auth
  explicitly. Auth is resolved in priority order:
  1. `--auth-config <path>` (or `TUMULT_MCP_AUTH_CONFIG`, default
     `~/.tumult/mcp-auth.toml` when present) — a TOML file granting each token a
     role (see below). This is the recommended production setup.
  2. `TUMULT_MCP_TOKEN` — a single static token, mapped to the **operator** role
     (backward-compatible with pre-RBAC deployments).
- **Two roles, fail-closed (default-deny).** Every tool is classified by its
  declared read-only hint:
  - **viewer** — may call read-only tools only (`tumult_chaosgraph_query`,
    `tumult_analyze`, `tumult_read_journal`, `tumult_compliance`,
    `tumult_fault_catalog`, `tumult_scaffold_experiment`, the `list_*` tools, …).
  - **operator** — may call **all** tools, including fault injection and
    execution (`tumult_run_experiment`, `tumult_gameday_run`,
    `tumult_create_experiment`, `tumult_report`, `tumult_gameday_create`,
    `tumult_recommend`).
  - **approver** / **admin** — higher-privilege roles shared with the wider
    platform; at the MCP gate they satisfy every operator requirement and
    therefore also call all tools.

  `admin` ⊇ `approver` ⊇ `operator` ⊇ `viewer`. A token absent from the
  config is **rejected, never elevated**; a missing or unknown role is a
  startup error; and a malformed config refuses every request rather than
  running open.
- **Auth config file format** (`~/.tumult/mcp-auth.toml`, mode `600`):

  ```toml
  [[tokens]]
  token = "<viewer-secret>"   # openssl rand -hex 32
  role  = "viewer"

  [[tokens]]
  token = "<operator-secret>"
  role  = "operator"
  ```

- **Terminate TLS at a reverse proxy.** The server speaks plain HTTP. Put
  nginx/Caddy/an Ingress in front to terminate TLS and, ideally, add a second
  auth layer (mTLS or an OIDC proxy). Never expose `:3100` directly to the
  internet.
- **TLS for the `tumultd` analytics daemon.** `tumultd` (OTLP/gRPC `:4317`,
  HTTP API/UI/OTLP `:4318`) can serve TLS directly — see §1a below. Without
  TLS configured it serves plaintext and logs a loud startup warning on any
  non-loopback bind; front it with a TLS-terminating proxy in that case.
- **SSH targets: never use `accept-any` in production.** tumult-ssh's
  `accept-any` host-key policy disables MITM protection and exists for
  throwaway lab targets only. Production experiments must use the default
  `verify` policy with a populated `known_hosts` (see
  [../plugins/tumult-ssh.md](../plugins/tumult-ssh.md)).
- **Rotate tokens by editing the config (or the secret) and restarting.** Issue
  a distinct token per principal so you can revoke one without disturbing the
  rest; rotate on a schedule and on any suspected exposure. Keep the file
  `600`-permissioned and out of version control.
- Clients pass the token as `Authorization: Bearer <token>` **and** in
  `_meta.authorization` on each `tools/call` (stdio clients rely on the latter).

## 1a. TLS for the `tumultd` analytics daemon

`tumultd` serves two listeners: OTLP/gRPC (`KRONIKA_OTLP_GRPC_ADDR`, default
`0.0.0.0:4317`) and HTTP — OTLP/HTTP ingest, the query API, and the web UI
(`KRONIKA_OTLP_HTTP_ADDR`, default `0.0.0.0:4318`). Both carry bearer tokens
and telemetry, so on any network-exposed bind they must be encrypted.

**Direct TLS (recommended for single-node).** Point both servers at a PEM
certificate chain and private key:

```bash
KRONIKA_TLS_CERT=/etc/tumult/tls.crt   # PEM certificate chain
KRONIKA_TLS_KEY=/etc/tumult/tls.key    # PEM private key (mode 600)
```

When both are set, the HTTP listener serves HTTPS (rustls) and the gRPC
listener serves TLS (tonic) with the same pair; the daemon validates the pair
at startup and **refuses to boot** on a missing file, malformed PEM, or
mismatched key. Setting only one of the two vars is also a startup error —
there is no silent fallback to plaintext. With direct TLS enabled, exporters
must use the `https://` scheme, and the daemon's own telemetry loopback is
moved to a plaintext listener on an ephemeral `127.0.0.1` port (loopback
only; nothing network-facing is plaintext).

Any CA-issued certificate works. For a lab/internal CA, or a throwaway
self-signed cert for evaluation:

```bash
openssl req -x509 -newkey rsa:2048 -nodes -days 90 \
  -keyout /etc/tumult/tls.key -out /etc/tumult/tls.crt \
  -subj "/CN=tumultd.internal" \
  -addext "subjectAltName=DNS:tumultd.internal,IP:127.0.0.1"
chmod 600 /etc/tumult/tls.key
```

**Reverse-proxy alternative.** If you already run nginx/Caddy/an Ingress,
leave `KRONIKA_TLS_*` unset, bind both listeners to `127.0.0.1`, and
terminate TLS at the proxy (gRPC needs HTTP/2 passthrough — e.g.
`grpc_pass` in nginx). This is also the answer when you want mTLS or an
OIDC proxy in front. With TLS unset, any non-loopback bind logs a loud
`TLS is OFF` warning at startup.

One caveat with a proxy: `tumultd` rate-limits logins by client IP, and
behind a proxy every request arrives from the proxy's address — so all
users share a single rate-limit bucket. That's the conservative option
(one slow brute-force lockout for everyone), and it's fine to accept. If
per-user limits matter, run without a proxy or let the proxy handle auth
itself. Note that `tumultd` deliberately does **not** trust
`X-Forwarded-For` — clients can spoof that header, which would let an
attacker reset their own bucket or pin the limit on someone else.

**Fail-closed ingest auth.** Independent of TLS: `tumultd` **refuses to
start** when either OTLP listener binds a non-loopback address without
`KRONIKA_INGEST_TOKEN` — an unauthenticated network ingest would accept
spoofed telemetry from anyone. Loopback binds stay open for local dev.

## 2. Deploy

**systemd** (`deploy/systemd/tumult-mcp.service`) — a hardened unit binding
localhost; front it with a TLS reverse proxy:

```bash
install -Dm600 /dev/stdin /etc/tumult/mcp.env <<'EOF'
TUMULT_MCP_TOKEN=<openssl rand -hex 32>
OTEL_EXPORTER_OTLP_ENDPOINT=http://your-collector:4317
EOF
cp deploy/systemd/tumult-mcp.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now tumult-mcp
```

**Kubernetes** (`deploy/k8s/tumult-mcp.yaml`) — Deployment + Service + PVC; the
token comes from a Secret. The template pins the release tag; verify the
published image digest for that release before applying it:

```bash
kubectl create secret generic tumult-mcp-token --from-literal=token="$(openssl rand -hex 32)"
kubectl apply -f deploy/k8s/tumult-mcp.yaml
```

The pod binds `0.0.0.0` (so the Service can reach it) — safe **only** because the
token is required. Expose externally through a TLS Ingress. Run a **single writer**:
`replicas: 1`, `strategy: Recreate` (see §4). The pod's working directory is
the persisted `/home/tumult/.tumult` volume, so generated definitions and
journals survive restart. A bounded `/tmp` volume supports temporary provider
files while the root filesystem stays read-only. The systemd unit similarly
uses its writable `/var/lib/tumult` state directory as the workspace.

This manifest deploys MCP only; it does not install the daemon, web UI, target
connections, or multi-tenant isolation. Verify health plus a harmless process
experiment, its persisted journal, and restart behavior in local Kubernetes
before using the same release on GKE.

**Health probes.** The health path differs per binary: `tumultd` / the ingest
servers answer `GET /healthz`, while `tumult-mcp` answers `GET /health`.
Configure liveness/readiness probes with the right path for the binary they
target — the paths are intentionally kept as-is for compatibility with
existing probes. `tumultd` additionally serves `GET /readyz` (readiness) and
`GET /metrics` (Prometheus text) — see §8 and §9.

## 3. Observability — bring your own collector

Telemetry is off unless you point it at a collector. Set
`OTEL_EXPORTER_OTLP_ENDPOINT` to your own OTLP/gRPC endpoint (usually `:4317`);
the built-in exporter uses gRPC. The daemon can also ingest OTLP/HTTP from
other clients on `:4318`. No collector is baked into the binary. Every experiment emits
`resilience.*` spans. The demo ships a SigNoz/collector stack as an *example* —
in production, point Tumult at whatever collector you already run
(see [observability-setup](observability-setup.md)).

## 4. The analytics store — single-writer model

A native DuckDB database permits a writing process to own the file. The
daemon's readers coexist with its writer **inside that process**. A separate
CLI or MCP process cannot open the same file while the daemon holds it, even
for read-only queries. Use daemon APIs, a separate database, or an exported
snapshot for independent consumers. Run one daemon replica with `Recreate`;
a shared PVC does not make multiple writers safe.

To ingest a CLI journal while the daemon owns the lake, configure
`TUMULT_DAEMON_URL` and an authorized `TUMULT_DAEMON_TOKEN`. Portable data export
uses `POST /api/lake/export`; read the files named in its committed manifest
rather than globbing historical Parquet files.

## 5. Backup & DR

Stop every writer before running a complete operational backup:

```bash
TUMULT_LAKE_PATH=/srv/tumult/lake.duckdb tumult store backup --output /secure-backups/tumult-20261008
tumult store restore --input /secure-backups/tumult-20261008 --output /srv/tumult/restored.duckdb
```

Backup copies the full database and writes a schema/version/checksum manifest.
Restore verifies the backup and refuses to overwrite an existing database.
Credentials and operational state are included, so protect the backup like
the live database. Back up external secrets, TLS keys, reports, org hierarchy,
plugin installations, attachment content and archives separately. Prove
restore in an isolated environment before relying on the backup policy.

Automatic retention is disabled: keep `KRONIKA_RETENTION_DAYS=0` and
`TUMULTD_RUN_RETENTION_DAYS=0`. Reports currently read hot data, so removing
archived history would make reports incomplete. Nonzero automatic retention
settings are rejected. [Data portability and recovery](data-portability.md)
defines archive exclusions, manifest handling and the complete restore path.

## 6. Blast radius — what actually limits impact

Two distinct fields:

- **`max_concurrent_faults`** is *enforced* by the runner — it caps how many
  background faults run at once. Set it to bound real impact.
- **`blast_radius`** is an *advisory* audit string (documents intent for the
  journal/compliance record). It does **not** enforce anything on its own.
- **Guards + auto-halt** are the live safety net: attach a guard probe (any
  HTTP/native/process check — e.g. a Prometheus SLO query) with a `min_breaches`
  debounce, and the experiment halts and rolls back when the guard trips. Wire
  guards to the same signals your paging uses.

## 7. Pre-flight checklist

- [ ] Auth configured — an auth config file (per-token roles) or `TUMULT_MCP_TOKEN`; each token a strong secret; rotation plan documented
- [ ] Least privilege: automation and read-only users hold **viewer** tokens; only operators hold **operator** tokens
- [ ] Server bound to localhost or behind a TLS-terminating proxy with auth required
- [ ] `tumultd`: `KRONIKA_TLS_CERT`/`KRONIKA_TLS_KEY` set (or a TLS reverse proxy in front) and `KRONIKA_INGEST_TOKEN` set on any non-loopback bind
- [ ] `OTEL_EXPORTER_OTLP_ENDPOINT` pointed at your collector
- [ ] Store volume persisted, encrypted, and on a backup schedule; single writer
- [ ] Experiments set `max_concurrent_faults` and attach guard probes to real SLOs

## 8. `tumultd` runtime flags (`TUMULTD_*`)

Daemon tunables are environment variables. The concurrency/timing knobs below
use positive defaults. Retention is an exception: its only supported value is
zero while reports cannot query archived history.

| Flag | Default | Effect |
|---|---|---|
| `TUMULTD_RUN_CONCURRENCY` | `2` | Experiments executing concurrently. Raise cautiously: each run injects real faults. |
| `TUMULTD_RUN_QUEUE_DEPTH` | `32` | Runs waiting for a worker. Enqueue beyond the depth is rejected (HTTP 429), never silently queued. |
| `TUMULTD_APPROVAL_SWEEP_S` | `60` | Seconds between approval-TTL sweeps (lapsed `pending_approval` runs become terminal). |
| `TUMULTD_SCHEDULE_TICK_S` | `30` | Seconds between scheduler ticks. Missed fires collapse per the scheduling policy; intervals under 60s are rejected server-side regardless. |
| `TUMULTD_WEBHOOK_TICK_S` | `15` | Seconds between webhook dispatcher ticks. |
| `TUMULTD_WEBHOOK_MAX_ATTEMPTS` | `5` | Consecutive failing ticks per endpoint before its pending events are moved to `webhook_dead_letters` and the cursor advances. |
| `TUMULTD_WEBHOOK_ENDPOINT_BUDGET_S` | `120` | Per-endpoint per-tick wall-clock budget; a hung receiver is abandoned at the budget and retried under backoff without stalling other endpoints. The per-request HTTP timeout is fixed at 2s. |
| `TUMULTD_GAMEDAY_TICK_S` | `15` | Seconds between GameDay supervisor ticks (campaign advancement). |
| `TUMULTD_RUN_RETENTION_DAYS` | `0` | Automatic deletion disabled; nonzero values are rejected to preserve report completeness. |
| `TUMULTD_RUN_RETENTION_TICK_S` | `3600` | Seconds between retention sweeps. |

**Escape hatches — demo/test only, never production:**

| Flag | Default | Effect |
|---|---|---|
| `TUMULTD_WEBHOOK_ALLOW_INSECURE` | off | Set `1`/`true` to allow `http://` webhook URLs (default is HTTPS-only). Plaintext delivers HMAC-signed audit events and the signing secret's protection is only as strong as the channel — do not enable outside a lab. |
| `TUMULTD_WEBHOOK_ALLOW_LOCAL` | off | Set `1`/`true` to allow loopback, private, and link-local (incl. cloud metadata `169.254.169.254`) IP-literal webhook URLs. This weakens the SSRF guard; enable only for local receivers in demos/tests. |

Related bootstrap knobs (schema/auth, documented in §1a and the
[platform walkthrough](platform-walkthrough.md)): `KRONIKA_INGEST_TOKEN`,
`KRONIKA_BOOTSTRAP_ADMIN_PASSWORD` (minimum 12 characters) and
`KRONIKA_BOOTSTRAP_TOKEN` (`kro_`-prefixed, minimum 20 characters after the
prefix). The bootstrap pair is a one-time demo/dev path — it is ignored once
any user exists, and production should run `tumultd create-admin` instead.

## 9. Daemon SLOs & alerting

`tumultd` exposes its own SLIs — separate from the experiment/product metrics
in `metrics/*.yaml` — via three endpoints on the HTTP listener. All three sit
behind the API auth middleware (Viewer); while the store has no users they
answer unauthenticated. Once auth is enabled, supply a Viewer API token to
health/readiness probes. Kubernetes HTTP probes treat only 200–399 as success;
401 is a failed probe. Use an exec probe that reads a mounted Secret or an
explicitly secured probe configuration. The demo Compose healthcheck sends
its bootstrap API token and requires a successful store check.

- **`GET /healthz`** — liveness: the single-writer channel round-trips and
  the store answers a probe query. 200 `ok` or 503 with the failing probe.
- **`GET /readyz`** — readiness: liveness plus schema migrations applied and
  at least one supervisor tick since boot (a dead supervisor task stops
  ticking). Use this for the readiness probe; use `/healthz` for liveness.
- **`GET /metrics`** — Prometheus text exposition of the daemon counters
  below. Scrape it like any other target.

**Daemon SLIs (all `tumultd_*`):**

| Metric | Type | Meaning |
|---|---|---|
| `tumultd_runs_started_total` | counter | Runs that began execution. |
| `tumultd_runs_completed_total` | counter | Runs that reached a completed terminal state (journal written). |
| `tumultd_runs_failed_total` | counter | Runs that failed (validation, dispatch refusal, runner error). |
| `tumultd_webhook_deliveries_succeeded_total` | counter | Webhook events delivered. |
| `tumultd_webhook_deliveries_failed_total` | counter | Webhook deliveries that failed (retried under backoff until dead-lettered). |
| `tumultd_webhook_dead_letters_total` | counter | Events abandoned to `webhook_dead_letters` — **permanent delivery loss**. |
| `tumultd_schedule_fires_total` | counter | Schedule fires. |
| `tumultd_active_campaigns` | gauge | GameDay campaigns currently advancing. |
| `tumultd_supervisor_last_tick_ns` | gauge | Last supervisor heartbeat, epoch ns. |

**Suggested alert thresholds** (tune to your traffic; chaos runs fail by
design, so rates matter more than single increments):

- **Daemon task died (page):** `time() - tumultd_supervisor_last_tick_ns / 1e9 > 120`
  (no supervisor tick for 2 minutes) or a failing `/readyz` for > 2 minutes.
- **Webhook delivery failure ratio (warn):**
  `rate(tumultd_webhook_deliveries_failed_total[15m]) / (rate(tumultd_webhook_deliveries_succeeded_total[15m]) + rate(tumultd_webhook_deliveries_failed_total[15m])) > 0.1` for 30m.
- **Webhook permanent loss (page):** `increase(tumultd_webhook_dead_letters_total[15m]) > 0` —
  events were dead-lettered; replay them from `run_audit` (the source of
  truth) once the receiver recovers.
- **Run failure rate (warn):** `rate(tumultd_runs_failed_total[1h]) / rate(tumultd_runs_started_total[1h]) > 0.25`
  for 1h — above this, check whether failures are daemon-level (validation /
  dispatch) rather than experiment outcomes.
- **Schedule fire stall (warn):** `rate(tumultd_schedule_fires_total[1h]) == 0`
  while enabled schedules exist — the scheduler is down or every schedule is
  broken; correlate with the heartbeat alert above.

A scrape config fragment:

```yaml
scrape_configs:
  - job_name: tumultd
    metrics_path: /metrics
    scheme: https          # or http behind your TLS proxy
    bearer_token: <viewer-token>   # once users exist; drop on loopback dev
    static_configs:
      - targets: ["tumultd.internal:4318"]
```

## Provider capabilities

| Runtime | Included | Deployment prerequisites |
|---|---|---|
| Source CLI/MCP | Shared process, script and native dispatch | Build `tumult-net` alongside the CLI/MCP for `tumult-net-proxyd`; keep plugin manifests/scripts discoverable and install their declared tools. |
| CLI/MCP image | Shared executors, bundled scripts/examples, TCP helper, Docker CLI and SSH client | Supply a writable workspace and explicit credentials/target access. Container-local paths are not host paths. |
| Daemon image | CLI, daemon/UI, script plugins, TCP helper, Docker CLI, SSH client and stress-ng | Authorize execution bindings; supply target credentials and external tools appropriate to the selected action. |
| MCP Kubernetes template | Authenticated MCP, persistent workspace, bounded temp directory | No target permissions or Docker socket are granted. Configure service accounts/RBAC, networking and secrets for the specific target. |

A provider appearing in `discover` means its implementation is available; it
does not establish access to a target or prove that dependencies are installed.
Docker actions require access to a Docker daemon; Podman actions need Podman.
CLI, MCP and daemon wire the k6 load executor. Load experiments still need a
compatible k6 binary and a readable script; missing tools or startup failures
reject execution before the fault method runs. Cloud and Windows
providers require their own credentials, platform and reachability. The demo
stack does not grant host Docker control automatically.

Before enabling a provider, verify its prerequisites and run a harmless probe
in the exact image and deployment identity. Then exercise a bounded fault and
rollback only against a disposable target. Source/unit tests and a healthy pod
are not a live-provider certification.

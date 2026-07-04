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

  `operator` ⊇ `viewer`. A token absent from the config is **rejected, never
  elevated**; a missing or unknown role is a startup error; and a malformed
  config refuses every request rather than running open.
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
- **Rotate tokens by editing the config (or the secret) and restarting.** Issue
  a distinct token per principal so you can revoke one without disturbing the
  rest; rotate on a schedule and on any suspected exposure. Keep the file
  `600`-permissioned and out of version control.
- Clients pass the token as `Authorization: Bearer <token>` **and** in
  `_meta.authorization` on each `tools/call` (stdio clients rely on the latter).

## 2. Deploy

**systemd** (`deploy/systemd/tumult-mcp.service`) — a hardened unit binding
localhost; front it with a TLS reverse proxy:

```bash
install -Dm600 /dev/stdin /etc/tumult/mcp.env <<'EOF'
TUMULT_MCP_TOKEN=<openssl rand -hex 32>
OTEL_EXPORTER_OTLP_ENDPOINT=http://your-collector:4317
EOF
cp deploy/systemd/tumult-mcp.service /etc/systemd/system/
systemctl enable --now tumult-mcp
```

**Kubernetes** (`deploy/k8s/tumult-mcp.yaml`) — Deployment + Service + PVC; the
token comes from a Secret:

```bash
kubectl create secret generic tumult-mcp-token --from-literal=token="$(openssl rand -hex 32)"
kubectl apply -f deploy/k8s/tumult-mcp.yaml
```

The pod binds `0.0.0.0` (so the Service can reach it) — safe **only** because the
token is required. Expose externally through a TLS Ingress. Run a **single writer**:
`replicas: 1`, `strategy: Recreate` (see §4).

## 3. Observability — bring your own collector

Telemetry is off unless you point it at a collector. Set
`OTEL_EXPORTER_OTLP_ENDPOINT` to your own OTLP endpoint (gRPC `:4317` or HTTP
`:4318`); no collector is baked into the binary. Every experiment emits
`resilience.*` spans. The demo ships a SigNoz/collector stack as an *example* —
in production, point Tumult at whatever collector you already run
(see [observability-setup](observability-setup.md)).

## 4. The analytics store — single-writer model

The persistent store (`~/.tumult/analytics.duckdb`, DuckDB) allows **one writer**.

- **One writer:** the running server (it ingests runs and refreshes derived data).
- **Readers coexist:** `tumult analyze`, `tumult chaosgraph query|neighbors`, and
  the MCP read tools open the store **read-only** and run concurrently with the
  writer.
- **Two writers conflict:** a second process opening for write (a CLI `tumult run`
  ingest, or a second server replica) gets a clear `StoreLocked` error. Do not run
  concurrent writers against one store — give CI its own `--store` path, or write
  through the single server. This is why the k8s Deployment is `replicas: 1` /
  `Recreate`.
- For heavy multi-consumer analytics, export to Parquet (`tumult store backup`)
  and query that from your warehouse instead of contending on the live store.

## 5. Backup & DR

The store is a plaintext file. Back it up with `tumult store backup <dir>`
(Parquet export) on a schedule, ship the output offsite, and host the volume on
encrypted storage. Restore is a fresh store re-ingesting journals, or querying the
Parquet archive directly.

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
- [ ] `OTEL_EXPORTER_OTLP_ENDPOINT` pointed at your collector
- [ ] Store volume persisted, encrypted, and on a backup schedule; single writer
- [ ] Experiments set `max_concurrent_faults` and attach guard probes to real SLOs

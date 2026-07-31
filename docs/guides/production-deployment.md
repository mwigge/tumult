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

The persistent store (`~/.tumult/lake.duckdb`, DuckDB) allows **one writer**.

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
- [ ] `tumultd`: `KRONIKA_TLS_CERT`/`KRONIKA_TLS_KEY` set (or a TLS reverse proxy in front) and `KRONIKA_INGEST_TOKEN` set on any non-loopback bind
- [ ] `OTEL_EXPORTER_OTLP_ENDPOINT` pointed at your collector
- [ ] Store volume persisted, encrypted, and on a backup schedule; single writer
- [ ] Experiments set `max_concurrent_faults` and attach guard probes to real SLOs

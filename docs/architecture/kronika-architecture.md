# Architecture — Krönika (the tumult platform stack)

Krönika is the daemon-and-UI half of the merged tumult platform: it ingests
chaos/resilience telemetry (OTLP + manual files), stores it in the unified
embedded DuckDB lake, executes registered experiment definitions under an
approval workflow, and serves the web UI plus compliance reports. It lives
in this repository as first-class `tumult-*` crates — imported from the
standalone kronika project and folded into the workspace (ADR-006); see
[Merge mapping and migration](#merge-mapping-and-migration-kronika--tumult)
for what became of each kronika crate and binary.

## Merge mapping and migration (kronika → tumult)

The standalone kronika repository was folded into this workspace (commits
`1227d23`, `d8fa169`, `d922f0b`). Code names map onto the merged tree:

| kronika (standalone) | tumult (this repository) |
|---|---|
| `kronikad` (package + binary) | `tumultd` (embeds `web/build/`) |
| `kronika-otel` | `tumult-otlp` |
| `kronika-store` | `tumult-lake` (unified store; tumult-analytics dissolved into it, schema v3+) |
| `kronika-ingest` | `tumult-ingest` |
| `kronika-metrics` | `tumult-metrics` |
| `kronika-report` | `tumult-report` |
| `kronika-docs` | `tumult-compliance` |
| `kronika-api` | `tumult-api` |
| `kronika-ai` | absorbed into `tumult-intelligence` as `llm` + `sql_guard` modules |
| `kronika-demo` binary | deleted — the demo is `docker/docker-compose.kronika.yml` |
| `web/` (SvelteKit SPA) | `web/` (unchanged location) |
| imported ADRs 0001–0005 | renumbered ADR-006…ADR-010 |
| `~/.kronika/kronika.duckdb` | `~/.tumult/lake.duckdb` |

Migration of pre-merge databases: `tumult store import-legacy
[--analytics-db <path>] [--kronika-db <path>]` merges an old
tumult-analytics store and/or a standalone kronika lake into the unified
store (idempotent natural-key dedupe; older schemas import via column
intersection — see the [CLI reference](../guides/cli-reference.md)). Store
path resolution: `TUMULT_LAKE_PATH` is canonical; `TUMULT_ANALYTICS_PATH`
and `KRONIKA_DB` remain as deprecated aliases for one release. The
`KRONIKA_*` daemon environment variables (`KRONIKA_HTTP_ADDR`,
`KRONIKA_INGEST_TOKEN`, `KRONIKA_LAKE_DIR`, …) keep their names — they are
the daemon's own configuration surface, not a store-location concern. The
old `kronika-legacy/` import directory and the
`docker/kronika-legacy-staging/` scaffold no longer exist; history
preserves them.

## Component diagram

```
                        ┌────────────────────────────────────────────┐
                        │                  tumultd                   │
                        │                                            │
 tumult ──OTLP/gRPC──▶  │  tumult-ingest (grpc :4317)                │
 (TUMULT_OTEL_ENABLED, │  tumult-ingest (http :4318, /v1/*,          │
  OTEL_EXPORTER_       │                          /healthz)          │
  OTLP_ENDPOINT)       │        │ decode prost (opentelemetry-proto) │
                       │        ▼                                    │
 smedja ──OTLP/HTTP──▶ │  tumult-otlp — pure translation:            │
 (SMEDJA_OTLP_         │  promote resilience.* + service.name to     │
  ENDPOINT, /v1/*)     │  columns, rest into MAP attrs               │
                       │        │ row batches                        │
 files (CSV / tumult   │  ManualImporter ─────────────┐              │
  journal JSON) ──────▶│  (tumultd import <file>)     │              │
                       │        ▼                     ▼              │
                        │  single bounded mpsc channel (backpressure) │
                        │    ▲ Batch::Exec — manual-evidence, run-    │
                        │    │ queue and approval closures from       │
                        │    │ tumult-api ride the same channel       │
                        │    │ (the API opens no write conn)          │
                        │        ▼                                    │
                        │  tumult-lake WRITER (single DuckDB conn)    │
                        └────────┬───────────────────────────────────┘
                                 │ exclusive write lock
                                 ▼
                    ┌─────────────────────────┐
                    │  ~/.tumult/lake.duckdb  (dir mode 0o700)      │
                    │  spans · logs · metric_sums/gauges/histograms │
                    │  import_batches · view: experiment_runs       │
                    │  manual_experiments · manual_experiment_audit │
                    │  evidence_attachments · users/sessions/tokens │
                    │  run_registry · runs · run_audit ·            │
                    │  approval_requests · approval_decisions (v7)  │
                    └────────┬─────────────────┘
              read-only conns (AccessMode::ReadOnly, many, coexist)
              ┌──────────────┼───────────────────────────┐
              ▼              ▼                           ▼
      tumult-metrics    tumult-report             tumult-api
      YAML semantic     digest renderer           JSON API (/api/*,
      layer → SQL       (ad-hoc + scheduled)      spawn_blocking)
              │              │                    │  overview · series ·
              │   tumult-compliance               │  experiments · runs ·
              │   compliance reports              │  approvals · logs ·
              │   (R1/R2/R3 → PDF +               ▼  traces · metrics ·
              │    HTML, scores)              web/ SPA (rust-embed, same
              │         │                     HTTP port) ──┘
              ▼         ▼                        ▲  POST /api/ask ──▶ tumult-
         <db dir>/reports/  (+ reports/v2/       │  intelligence::llm →
         report_<epoch>.html  KRK-*.pdf/.html/.json)  sql_guard → read-only
         (store closed)                          │  execution (guarded, LIMIT)
```

## Data flow

1. **OTLP in** — gRPC (tonic) and HTTP (axum, `application/x-protobuf`)
   servers receive `Export{Trace,Metrics,Logs}ServiceRequest`s.
2. **Normalize** — `tumult-otlp` converts proto values to plain row structs,
   promoting the tumult metadata standard's (`resilience.*` v2.0)
   low-cardinality keys and `service.name`/`service.version` into wide-table
   columns; dynamic keys stay in `MAP(VARCHAR, VARCHAR)` columns.
3. **Store** — batches travel a bounded tokio mpsc channel (batching +
   backpressure) to the single writer connection.
4. **Semantic layer** — `metrics/*.yaml` definitions compile to strictly
   validated SQL (`[a-z0-9_.]` identifiers only → injection-impossible).
5. **UI / reports** — `tumult-api` answers the UI's queries through fresh
   read-only connections (never touching the write lock); the SPA itself is
   rust-embedded into tumultd and served from the same HTTP port. Beyond
   Overview/Experiments it backs the explorer pages: `/api/logs[ /volume]`
   (raw log search + severity volume), `/api/traces[ /durations, /{id}]`
   (spans grouped into traces, duration percentiles, per-trace detail),
   `/api/metrics/catalog` + `/api/metrics/query` (raw sums/gauges/histograms
   with optional attribute grouping; histogram p95 interpolated in Rust) and
   `/api/topology` (service/target call graph). Mutating routes exist but
   stay read-only in spirit: they ride the single-writer channel (run
   control below, manual-evidence lifecycle, approval decisions). Ad-hoc
   digests come from `tumultd report` / `GET /report`; with
   `KRONIKA_REPORT_INTERVAL` set, the daemon additionally renders a digest
   per interval into `<db dir>/reports/` (surfaced by `/api/reports`). When
   an LLM is reachable, digests (scheduled and `POST /api/reports/generate`)
   gain a narrative section via `tumult_report::narrative`, which keeps
   only sentences whose numbers are grounded in the report's own facts.

The docker demo (`docker/docker-compose.kronika.yml`) is the **reference
ingestion flow** end to end: a pinned tumult binary runs the
experiment suite and emits genuine OTLP/gRPC (traces, metrics, logs) into
tumultd, which normalizes, stores and renders it into the HTML reports
under `demo-out/`. Whatever tumult emits on the wire is exactly what
Krönika's semantic layer computes over.

## Single-writer model (tumult-lake)

DuckDB is single-writer per file; a read-write open holds an exclusive lock.

- **One writer per process** — every ingest path funnels through the channel
  onto one `Writer`. A second read-write open maps the opaque DuckDB lock
  error to `StoreError::StoreLocked` (after a short bounded retry).
- **Many readers** — `AccessMode::ReadOnly` connections do not take the write
  lock, so reports and the UI API coexist with the writer *inside the daemon
  process*. Cross-process, DuckDB permits only one process with the file open
  read-write: `tumultd report` therefore requires the daemon to be stopped,
  and the live `GET /report?metric=<name>` endpoint exists precisely so
  reports can be produced while the daemon holds the store.
- **At rest** — DuckDB has no encryption at rest; the store directory is
  created `0o700` and should sit on an encrypted volume for sensitive data.

## Daemon-run experiments (schema v5)

The daemon executes experiments itself, not only ingests their telemetry
(ADR-011). `POST /api/runs/validate` applies the CLI's exact
parse/resolve/validate pipeline (`tumult_ingest::prepare_run`) and
registers the definition content-hash-deduped in `run_registry`;
`POST /api/runs/dry-run` previews the resolved plan; `POST /api/runs`
enqueues onto a bounded in-process queue (`TUMULTD_RUN_CONCURRENCY` /
`TUMULTD_RUN_QUEUE_DEPTH`, 429 on overload); `POST /api/runs/{id}/stop`
cancels the runner's e-stop token mid-method (rollbacks unwind the fault)
or cancels before start; `GET /api/runs[/{id}]` read state and audit
trail. Execution goes through `tumult-exec`'s `ProviderExecutor` — the
same crate the CLI uses — on a fixed worker pool, with every state
transition persisted through the single-writer channel (`runs` +
`run_audit`, deliberately index-free since schema v5: ART index desync
after SIGKILL made crash-time UPDATEs fail fatally). At startup, runs
left active by a previous process lifetime are reconciled: marked
`orphaned`, rollbacks attempted (`run_orphan_rollback`) even when the
state writes themselves fail, outcome recorded (`rollback_pending` flags
the failures for an operator). A telemetry loopback points the daemon's
own OTel exporter at its own gRPC ingest, so daemon-run experiments land
in the same tables and UI as CLI runs. The UI drives the whole loop: the
Run page (`/runs/new`) picks a validated definition from
`GET /api/registry[/{id}]`, renders a parameter form from the
definition's `${var}` placeholders, previews the resolved plan
(`dry-run`), and enqueues; the run detail (`/runs/[id]`) polls state,
embeds the live waterfall as loopback spans land, and exposes the
two-step e-stop with rollback status — all role-aware (viewer is
read-only).

## Authentication and RBAC (schema v6)

The API authenticates once any real user exists (ADR-012) — until then it
behaves exactly as before, so upgrades and loopback dev are unaffected.
Primitives live in the shared `tumult-auth` crate (also used by the MCP
server): argon2id password hashing at OWASP parameters, opaque ids, and
the `host_is_loopback` bind policy. Browser sessions are 256-bit opaque
cookies (`HttpOnly`, `SameSite=Strict`, `Secure` off loopback, 12h);
automation uses `kro_`-prefixed bearer tokens. Both are stored only as
sha256 hashes in the index-free v6 auth tables (`users`, `sessions`,
`tokens`, `user_env_scopes`) behind the single writer. Authorization is a
middleware over a single route table (`viewer < operator < approver <
admin`; unmatched routes fail closed to admin) plus optional per-user
environment scopes that filter experiment/run visibility. Run-audit
events and manual-evidence mutations record the authenticated username;
pre-auth free-text actors are attributed to a disabled `legacy` backfill
user seeded by the migration. Bootstrap: `tumultd create-admin` (one-time
password, `must_change` at first login) with the MCP-style guard — a
non-loopback bind refuses to start unauthenticated.
`KRONIKA_INGEST_TOKEN` guards OTLP `/v1/*` HTTP and gRPC ingest;
clients authenticate with the standard `OTEL_EXPORTER_OTLP_HEADERS`, and
the CLI sends `TUMULT_DAEMON_TOKEN` on journal import.

## Approval workflows and hash pinning (schema v7)

Run creation is change management (ADR-013). `POST /api/runs` resolves
and validates the definition at request time and classifies it into a
risk tier from frozen facts (env class, fault kinds, rollback presence,
destructive-name heuristic, probe-only): T0 (catalog hash or probe-only)
enqueues directly; T1/T2/T3 park in `pending_approval` behind an
approval request that pins the resolution inputs (SHA-256 over
`{definition_toon, params, env, target}` — the inputs, not the resolved
artifact, whose `HashMap` fields serialize nondeterministically). Quorum
is 1 approver (T1/T2) or 2 (T3) with writer-enforced segregation of
duties (approver ≠ requester, one decision each); approvals lapse (T1
72h, T2 24h, T3 4h — swept to terminal `expired`, re-request only) and
are single-use (consumed at dispatch). T3 approvals re-run the
tumult-autopilot gate in-process against current ambient facts
(`KRONIKA_AUTOPILOT_POLICY`, fail-closed unset); a Veto is never
approval-overridable. Break-glass (admin, mandatory justification)
bypasses quorum and TTL — never the pin, which the worker re-verifies at
the last moment (`dispatch_refused` on drift) — and opens a
retrospective manual-evidence draft as compliance debt. `run_audit`
gains a per-run hash chain (`prev_hash`/`new_hash`,
`verify_run_audit_chain` for tamper detection) with the authenticated
actor on every transition. Surfaces: `GET /api/approvals` +
approve/reject/break-glass endpoints (route-table roles), the
`/approvals` queue page and run-detail chain card in the UI, and an
"Approval chain (change management)" section in the R2 evidence pack
(SOC 2 CC8.1).

## Parquet lake + retention (durability story)

Two tiers, one guarantee: **nothing leaves the hot store before an
immutable copy exists in the lake.**

- **Hot tier** — the embedded DuckDB store: ACID, single-writer, WAL-backed
  (crash-safe to the last committed batch). Optimised for the recent-query
  workload of the UI and reports.
- **Cold tier** — the parquet lake (`KRONIKA_LAKE_DIR`, default
  `<db dir>/lake`): per table, one write-once file per day-partition
  (`spans/date=2026-07-29/data-<run>.parquet`). Files are never rewritten —
  *immutability as a compliance feature*: next to the v0.5.0 hash-chained
  manual-evidence audit, the trail of what Krönika recorded is
  WORM-shaped and tamper-evident, and readable by any parquet-capable
  tool (`read_parquet('lake/spans/date=*/*.parquet')`).
- **Export** — incremental against a per-table event-time watermark in
  `<lake>/_meta.json` (tmp+rename, advanced only after every table
  succeeded → idempotent retries). `manual_experiments` exports as a full
  snapshot per run (records mutate through their review lifecycle;
  fingerprint-gated so an unchanged register writes no new file); its
  audit table exports incrementally on `changed_at_ns`. Runs on
  `KRONIKA_LAKE_INTERVAL` (default `24h`) or on demand via
  `POST /api/lake/export`; `GET /api/lake/status` shows watermarks, files
  and bytes.
- **Retention** — `KRONIKA_RETENTION_DAYS=0` (default) keeps everything.
  When >0, hot rows older than the cutoff are deleted **only if already
  exported** (`ts_ns <= watermark`), through the single-writer channel.
  `manual_experiment_audit` and `manual_experiments` are never deleted:
  append-only compliance evidence in both tiers.

Caveat (event-time watermarking): rows arriving with `ts_ns` at or below
the current watermark are invisible to incremental export — irrelevant for
real-time telemetry; re-export from scratch after hand-backfills. See
ADR-010.

## Schema v2

Wide, ClickHouse-exporter-aligned tables: `spans`, `logs`, `metric_sums`,
`metric_gauges`, `metric_histograms` (+ `import_batches` for manual imports,
`schema_meta` versioning). Rollup view `experiment_runs` projects one row per
`resilience.experiment` span.

v2 adds manual evidence: `manual_experiments` (content + draft → submitted →
verified/rejected lifecycle + provenance + `content_hash`),
`manual_experiment_audit` (append-only, `prev_hash → new_hash` hash chain),
and `evidence_attachments` (external URIs only). The org hierarchy is not a
table — it is `org.yaml` (`KRONIKA_ORG_FILE`, default `<db dir>/org.yaml`)
loaded at daemon start; org rollups are computed at read time from the
latest-run scoring SQL (see ADR-009).

## Roadmap: external tooling on the lake

The parquet lake (above) is the substrate for future external tooling —
any parquet-capable engine can query Krönika's history without touching
the hot store.

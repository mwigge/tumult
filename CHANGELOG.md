# Changelog

All notable changes to the Tumult project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

## [2.21.0] — 2026-08-06

**The GUI grows up, and Krönika becomes the analytics half for real.** The
web UI is now a full chaos console: author experiments in the browser from
the live fault catalog (catalog → wizard → validate → dry-run → run),
schedule them, wire signed webhooks, run GameDay campaigns, and halt
everything with a two-click kill switch — all under tiered approvals and a
hash-chained audit. Krönika's OTLP→DuckDB lake gains run-system backup,
retention, and daemon self-observability (`/metrics`, `/readyz`, deep
`/healthz`), plus verified Grafana and SigNoz reference stacks. A full
security, architecture, SRE and product review accompanied the work; every
finding shipped in this release.

### Added
- Website visual rework: the docs site gains a `tumult` color scheme for
  just-the-docs derived from the product UI palette
  (`web/src/lib/theme.css` — graphite chrome, blue accent), custom
  typography/spacing polish, and a reworked landing page that leads with
  the current platform — web-UI authoring, T0–T3 approvals and the stop-all
  kill switch, the Kronika lake, compliance evidence, and the 40-tool MCP
  surface — with eight current-SPA screenshots embedded from
  `docs/assets/`.
- Recaptured all thirteen `docs/assets/` screenshots on the current SPA
  (new nav: Author, GameDays, Events, Webhooks, Users, Schedules; Halt all
  on Runs) and added eight new captures: `author-catalog.png`,
  `author-new.png`, `runs-stop-all.png`, `schedules.png`, `events.png`,
  `webhooks.png`, `users.png`, `gamedays.png`. The platform walkthrough
  gains an "Operate the platform" section covering them, and the README
  screenshot table references the Author and Runs pages.
- Web UI `/users` admin page over the existing admin API (`/api/users*`):
  create users (generated one-time password shown once), change roles, edit
  environment scopes, reset passwords, disable/re-enable accounts; admin-only
  nav entry (everything shows in open local mode).
- `scripts/ui-authoring-check.sh`: curl-based smoke test for the web-UI
  authoring flow (SPA shell, non-empty catalog, scaffold, validate-and-
  register + dedup, dry-run, validation-error path, probe-as-action 400,
  and viewer-vs-operator RBAC), runnable against any running tumultd with
  env-overridable URL/token. The kronika compose stack mounts `plugins/`
  into tumultd with `TUMULT_PLUGIN_PATH` so the authoring catalog is
  populated there.
- `/author/new` authoring wizard in the web UI: a parameter form generated
  from the chosen catalog action's documented arguments (title, target,
  steady-state probe as command or HTTP URL), one-click TOON scaffolding
  via `POST /api/authoring/scaffold`, an editable TOON editor with a
  "Validate & register" step (Operator-gated server-side, content-hash
  deduped), and a handoff into the New Run launcher
  (`/runs/new?registry_id=…`). New `ActionParamForm` and `ToonEditor`
  components.
- `/author` page in the web UI: a browsable, searchable fault catalog
  (domains → actions/probes with their documented arguments) backed by
  `GET /api/authoring/catalog`, with an "Author" nav entry and an empty
  state that explains plugin discovery paths. Picking an action deep-links
  into the authoring wizard (`/author/new`).
- Web client plumbing for experiment authoring: typed wrappers in
  `web/src/lib/api.ts` for `GET /api/authoring/catalog`,
  `POST /api/authoring/scaffold`, and `POST /api/runs/validate`
  (`api.catalog` / `api.scaffold` / `api.validateToon`) with matching
  response types, and a `?registry_id=` deep link on the New Run page that
  preselects a registered definition — the handoff target for the upcoming
  in-UI authoring flow.
- Authoring REST endpoints in `tumult-api`: `GET /api/authoring/catalog`
  serves the live fault catalog from plugin discovery (domains → actions →
  documented args), and `POST /api/authoring/scaffold` generates validated
  experiment TOON from a catalog action and its arguments. Same code paths
  as the MCP `tumult_fault_catalog` / `tumult_scaffold_experiment` tools,
  exposed without an MCP hop so the web UI can author experiments
  in-process. Both are Viewer-level in the RBAC route table and persist
  nothing — registration stays behind Operator-level
  `POST /api/runs/validate`.
- Token management on the same page, backed by the new
  `GET /api/tokens` admin endpoint (newest first, owners' usernames, revoked
  included, never the hashes): list with active/expired/revoked status, mint
  (plaintext shown once, optional day-based expiry), revoke.
- `POST /api/users` now enforces the same ≥12-character minimum on a
  supplied password as the admin reset endpoint (400 otherwise).
- `POST /api/runs/dry-run` gains an additive `plan.scope` block for the
  blast-radius preview: the declared `blast_radius` note, the
  fault-injecting method steps with the provider identity and only the
  target-identifying arguments (container/host/selector/process/interface/
  pod/namespace), the guards (name, probe, `min_breaches`), and
  `max_concurrent_faults` — always present, nulls/empty when undeclared.
- Blast-radius preview in the web UI: a shared `ScopeSummary` component
  renders the scope block (note, targeted fault actions, guards,
  concurrency cap) on the `runs/new` dry-run panel — where the launch
  button reads "Start run — affects N target(s)" — and on the run detail
  page (resolved from the run's own definition and parameters).
- Global halt: `POST /api/runs/stop-all` (Operator) e-stops every active
  run — running experiments cancel at the next activity boundary and roll
  back, queued and approval-parked runs are cancelled before start — with
  the halting principal audited on each run's `stop_requested` event
  (including the previously unaudited cancel-before-start path). The Runs
  page gains a two-step arm/confirm "Halt all" kill switch.
- Scheduling core (schema v10): the `run_schedules` table plus the daemon's
  schedule scheduler — interval-based recurring runs fired through the
  normal run path (tier classification and approval gating preserved, so a
  scheduled production run still parks for approval) with actor
  `schedule:<name>`. Interval semantics (`interval_s`), not cron — the
  workspace has no cron parser; missed fires during downtime collapse into
  one, a full run queue retries next tick. `TUMULTD_SCHEDULE_TICK_S`
  (default 30s) sets the tick. CRUD API and UI follow separately.
- Schedule CRUD API: `GET /api/schedules` (Viewer, definition name joined),
  `POST /api/schedules` (Operator — interval bounds 60s–30d, name ≤100
  chars, registry id must resolve 404, definition must resolve with the
  supplied vars 400), `POST /api/schedules/{id}/enable` and
  `/api/schedules/{id}/delete` (Operator, 404 on unknown id). Schedules
  start enabled; creation records the principal.
- Web UI `/schedules` page: list with interval, next-fire countdown,
  last-run link, enabled badge and created-by; create form (registry
  picker, interval presets, env, optional JSON vars); enable/disable and
  arm/confirm delete. Operator-only nav entry.
- Event feed: `GET /api/events` (Viewer) — every run's audit events from
  the hash-chained `run_audit`, newest first, with `run_id` / `event`
  filters, a `before` cursor and a 200-row cap, hash-chain links included;
  plus the `/events` UI page polling every 5s with event-type filter
  chips. Environment scoping matches the run list.
- Webhooks (schema v11): admin-managed outbound run-event notifications.
  `/api/webhooks*` CRUD (Admin; the HMAC secret is returned exactly once
  at creation), a daemon dispatcher posting signed
  (`X-Tumult-Signature: sha256=<hmac-sha256>`) JSON per webhook cursor —
  fire-and-log with one retry, so a down receiver misses events rather
  than blocking runs — and a `/webhooks` admin page. SSRF policy: https
  only; http and loopback/private/link-local addresses need the explicit
  `TUMULTD_WEBHOOK_ALLOW_INSECURE=1` / `TUMULTD_WEBHOOK_ALLOW_LOCAL=1`
  opt-ins. `TUMULTD_WEBHOOK_TICK_S` (default 15s) sets the dispatch tick.
- GameDay registration (schema v12): `run_registry.kind` distinguishes
  gameday definitions from experiments. `POST /api/gamedays/validate`
  registers a campaign TOON plus its referenced experiment TOONs (each
  validated through the normal run pipeline and content-hash deduped as
  ordinary runnable definitions), storing the resolved
  path→registry-id envelope; `GET /api/gamedays` and
  `GET /api/gamedays/{id}` list campaigns and return the parsed plan
  (scoring, regulatory mapping, ordered steps). Campaign execution
  follows separately.
- GameDay execution: `POST /api/gamedays/{id}/runs` (Operator) starts a
  campaign as a parent run, and the daemon's gameday supervisor advances
  it through its experiments as sequential child runs
  (`runs.gameday_id`) — each step through the normal run path, so a gated
  experiment parks the campaign at an approval. The parent takes the
  campaign outcome: `failed` on any failed/orphaned child, else `passed`
  when the passed fraction meets `scoring.pass_threshold`, else
  `deviated`. `GET /api/runs?gameday_id=` lists a campaign's children.
  `TUMULTD_GAMEDAY_TICK_S` (default 15s) sets the supervisor tick. Shared
  k6 load and GameDayJournal ingest remain deferred.
- GameDay UI: `/gamedays` lists campaigns and registers new ones (campaign
  TOON + experiment TOON map); `/gamedays/{id}` shows the parsed plan
  (steps with compliance maps, regulatory mapping, scoring), launches
  campaigns (operator), and watches the newest campaign's children
  progress with per-step status badges.
- docs: two getting-started blog posts — `26-web-ui-getting-started.md`
  (the governance half: tumultd bootstrap, RBAC login, registry → approvals
  → run → e-stop → audit, the authoring wizard, schedules, GameDays,
  webhooks, events, users) and `27-kronika-getting-started.md` (the
  analytics half: OTLP ingest, the single-writer DuckDB lake, parquet
  export, semantic metrics, compliance reports, and Grafana/SigNoz
  alongside); blog index updated. `scripts/check-docs.py` now expects 27
  posts and validates that every post carries an `updated:` review date
  instead of one hardcoded editorial date.
- `scripts/ui-table-stakes-check.sh` (`make ui-check`): demo-check-style
  end-to-end smoke for the UI table-stakes track against a running
  tumultd — users + RBAC boundary, tokens, dry-run scope, global halt with
  audit actor, schedule CRUD plus a real scheduler fire, the events feed,
  a gameday campaign's first child, and a signed webhook delivery verified
  against a local receiver (HMAC-SHA256). Env-overridable
  (`TUMULTD_URL`, `SMOKE_ADMIN_TOKEN`), idempotent across reruns.
- Grafana full-stack reference implementation: `docker/docker-compose.grafana-full.yml`
  boots otelcol-contrib + Tempo + Mimir + Loki 3.x + Grafana (pinned
  versions, named volumes) wired to `collector/otel-collector-grafana.yaml`,
  with provisioned Tempo/Mimir/Loki datasources and a reference dashboard
  (`tumult-grafana-full.json`). How-to in `docs/guides/grafana-stack.md`,
  including the OTLP→Prometheus metric-name translation table verified
  live against Mimir.
- SigNoz bulk import: `scripts/signoz-bulk-import.sh` backfills spans from
  the Parquet lake straight into SigNoz's ClickHouse (`signoz_index_v3` plus
  the `traces_v3_resource` companion table, so the UI resolves services),
  bypassing the collector. Docker-exec and local `clickhouse-client`
  transports, `--dry-run` per-partition counts, and a file ledger for
  idempotent incremental re-runs (`signoz_index_v3` is a plain MergeTree —
  no engine-level dedup). How-to in `docs/guides/signoz-bulk-import.md`,
  with the full lake→SigNoz column mapping, verified live against the
  `signoz-standalone` ClickHouse.
- Daemon self-observability: `/healthz` now probes the writer channel and
  the lake store (a static "ok" before), joined by `/readyz` (schema
  version + supervisor heartbeat) and a Prometheus-text `/metrics`
  endpoint with counters for runs, webhook deliveries, schedule fires and
  active campaigns — all Viewer-gated like `/report`.
- Run retention: terminal runs and audit rows older than
  `TUMULTD_RUN_RETENTION_DAYS` (default 90) are swept every
  `TUMULTD_RUN_RETENTION_TICK_S` (default 3600s) — the `runs`/`run_audit`
  tables no longer grow monotonically.
- Run-system backup: lake export now covers `runs`, `run_audit`,
  `run_schedules`, `webhooks` (+ approvals and users), and
  `tumult store backup` opens the store read-only so it works against a
  live daemon instead of failing on the write lock.
- tumultd deploy artifacts: `deploy/systemd/tumultd.service` and
  `deploy/k8s/tumultd.yaml` (PVC, liveness → `/healthz`, readiness →
  `/readyz`, all `TUMULTD_*` knobs documented) — deploy/ previously only
  shipped tumult-mcp.

### Fixed
- `/author` catalog page crashed with Svelte `each_key_duplicate` whenever
  two mounted plugins exposed same-named actions in one domain (e.g.
  `tumult-db-mysql`/`tumult-db-postgres` `kill-connections`,
  `tumult-containers`/`tumult-pumba` `pause-container`) — the each-block now
  keys on `plugin::name`.
- docs: follow-up review pass — the scheduling guide now describes the
  shipped schedules feature (JSON API, web UI page, 60s interval floor,
  audited `schedule:<name>` runs, `TUMULTD_SCHEDULE_TICK_S`), QUICKSTART
  gains a "Choosing your path" section and the correct experiment-format
  link, README lists the current web UI pages and adds a "How Tumult
  differs" section, the demo contract is bumped to 2.21 and lists all 13
  demo experiments (the topo pair is also scoped out in `demo/README.md`),
  and `gamedays/` gets a README.
- Docs accuracy pass after a full user-journey audit: README badge and
  kronika "Try it" steps (`.env` is required now), real GameDay sample
  output in QUICKSTART, `trend`/`report`/`run`/`analyze`/`mcp serve`/
  `coverage-gaps` flag gaps in the CLI reference, missing
  `version`/`guards`/`blast_radius`/`max_concurrent_faults` fields in the
  experiment format guide, stale crate counts and unsafe-block claims on
  the website, and a Linux one-liner in the release notes.
- CLI help texts pointed at the old `~/.tumult/analytics.duckdb` default;
  they now say `~/.tumult/lake.duckdb` (and `TUMULT_LAKE_PATH`).
- `tumult validate` on a GameDay file now hints to use
  `tumult gameday run` instead of a bare unknown-field error.
- GameDay campaign launches are now atomic: the active-campaign check and
  the parent-run insert happen inside the same single-writer batch, so two
  concurrent launches can no longer both win the 409 race.
- Daemon restarts no longer destroy running GameDay campaigns: orphan
  reconciliation excludes campaign parents (they got a nonsense rollback
  of their JSON envelope before), and a restart-mid-campaign integration
  test proves the supervisor resumes them.
- Webhook delivery is reliable: per-endpoint dispatch isolation (a dead
  endpoint can no longer stall the dispatcher ~400s/tick), cross-tick
  exponential backoff (`TUMULTD_WEBHOOK_ENDPOINT_BUDGET_S`,
  `TUMULTD_WEBHOOK_MAX_ATTEMPTS`), and dead-lettering
  (`webhook_dead_letters`, schema v13) instead of silently advancing the
  cursor past failed deliveries.
- Stop-all no longer halts partially: it continues past per-run store
  errors and always returns 200 with a `{requested, stopped,
  skipped_terminal, failed}` summary.

### Changed
- `collector/otel-collector-grafana.yaml`: replaced the deprecated contrib
  `loki` exporter with `otlp_http` against Loki's native OTLP endpoint
  (`/otlp`), renamed exporter kinds to their current canonical names
  (`otlp_grpc`, `prometheus_remote_write`, `otlp_http`), dropped the
  no-op `tls.insecure` on the remote-write exporter, and added the
  `health_check` extension. Verified against otelcol-contrib 0.157.0.
- Large-file splits (the ~400-line convention): `tumult-api` (lib.rs,
  auth, and the 3,800-line integration suite), `tumultd` (main →
  serve/admin/reports/lake_jobs), `tumult-ingest` runs queue,
  `tumult-compliance` builders, `tumult-lake` (manual/store/lake/auth/
  approvals/runs), `tumult-cli` clap modules, and the two biggest web UI
  pages into components. Inline test modules moved out across
  `tumult-core`, `tumult-mcp`, `tumult-baseline` and `tumult-otel`. Pure
  code motion — no behaviour changes.
- `tumult-mcp` binary now uses clap like the other binaries (usage
  errors exit 2, `--help` on stdout, unknown flags rejected; adds
  `--token` and `--version` for parity with `tumult mcp serve`).
- `topology lineage`/`recommend` share one `TopologyFormat` enum with
  `topology map`; `mermaid` is rejected with a clear message outside
  `map`.
- `tumult-cypher` emits a `cypher.query` span with result attributes —
  no engine crate is uninstrumented anymore.
- Plugin input validation: `tumult-containers`, `tumult-kafka` and
  `tumult-process` scripts now source `plugins/lib/validate.sh` and fail
  fast on invalid input before any destructive command runs.
- Cohesion fixes: consistent `DuckDb` casing and prefixed error messages
  in `tumult-lake`, one help-text style across the CLI, documented
  env-var prefixes (`KRONIKA_*`/`TUMULTD_*`/`TUMULT_*`), telemetry naming
  conventions, store/lake glossary, and the `/healthz` vs `/health`
  probe-path difference.
- Scaffold/catalog orchestration now lives in `tumult-authoring` and is
  shared by the REST and MCP surfaces (which had diverged — the MCP tool
  now rejects probe-kind actions like the API); shared error helpers,
  registration dedup and tick-loop scaffolding extracted across
  `tumult-api`/`tumult-ingest`. Pure code motion beyond the probe-rule
  alignment.

### Security
- `POST /api/runs/dry-run` is Operator-gated: the resolved plan carries
  substituted `${secrets.*}` values (and a templated secret-file path was
  an arbitrary-file-read oracle) — Viewer tokens now get 403.
- Environment scopes are enforced on mutations, not just reads:
  `POST /api/runs`, `/api/schedules` and `/api/gamedays/{id}/runs` reject
  a caller-chosen `env` outside the principal's scopes with 403 (a
  staging-scoped operator could previously launch — but not see or stop —
  runs in production).
- `/report` is served behind the same auth middleware as `/api` — store
  analytics were reachable unauthenticated on hardened deployments.
- Bootstrap secrets have an entropy floor (`KRONIKA_BOOTSTRAP_TOKEN` ≥20
  chars after `kro_`, admin password ≥12), validated before any write.
- Changing or resetting a password now revokes that user's sessions and
  tokens in the same writer transaction.
- Webhook deliveries gain replay protection: an additive
  `X-Tumult-Timestamp` + `X-Tumult-Signature-V2` (HMAC over
  `{timestamp}.{body}`, ±5 min tolerance) — the original
  `X-Tumult-Signature` header is byte-for-byte unchanged, so existing
  receivers keep working.

## [2.20.0] — 2026-07-31

**Security hardening across the board, and one product everywhere.** A
full code and security review drove this release: every finding is either
fixed here or documented with a reason. All user-facing strings, the docs
and the website now present a single product — Tumult — in plainer
language.

### Added
- Optional TLS for `tumultd`: set `KRONIKA_TLS_CERT`/`KRONIKA_TLS_KEY`
  (PEM paths) to serve HTTPS on the API/UI listener and TLS on the gRPC
  ingest listener. Plaintext binds beyond loopback log a loud warning;
  the production deployment guide covers direct TLS and reverse-proxy
  setups.
- Login rate limiting: token bucket per IP+username on
  `POST /api/auth/login` (429 on bursts, failures audit-logged, a
  successful login resets the bucket).
- Optional `kro_` API-token expiry (`expires_at_ns`, store schema v9);
  expired tokens authenticate exactly like revoked ones.
- Environment scoping on reports: scoped users generate reports confined
  to their environments, and can only list/download artifacts whose
  recorded coverage lies within their scopes. Legacy artifacts without
  coverage metadata fail closed for scoped principals.
- Natural-language analytics hardening: a function allowlist in the SQL
  guard plus a locked-down reader (`enable_external_access=false`) —
  DuckDB file-read functions (`read_text`, `read_blob`, `glob`, …) are
  rejected, closing an env-secret exfiltration path.
- LLM prompt-input sanitisation in the agentic layer (bidi/zero-width/
  control characters stripped, length caps).
- `.env.example` for the demo stacks; the compose files fail fast with a
  clear message when required secrets are unset.
- A specific hint on the ask page when a question can't be answered
  within the user's environment scopes (HTTP 422).
- Open-auth mode (zero users) logs a prominent startup warning.

### Changed
- Environment scoping now enforced on every telemetry endpoint
  (overview, logs, traces, metrics, timeseries, topology, scores,
  reports) — scoped principals are fail-closed where data has no
  environment linkage.
- 5xx responses return generic bodies; internal error details are only
  logged server-side.
- OTLP ingest refuses to start on non-loopback binds without
  `KRONIKA_INGEST_TOKEN` (was warn-only).
- Unified outlier metric names onto the `tumult.*` convention
  (`baseline.*`, `script.*`, `resilience.agent.*` renamed — update custom
  dashboard queries).
- User-facing strings rebranded to Tumult: CLI help, web UI title and
  sidebar, report titles, and PDF/HTML metadata ("Generated by Tumult",
  "Source: Tumult").
- Docs and the tumult.rs landing page rewritten as one product, in
  plainer language; the site is restructured around the end-to-end
  platform story (define → run → store → explore → report → recommend).
- `anyhow` removed from library crates: `tumult-tui` (`TuiError`) and
  `tumult-intelligence` (`RecommendError`) now use `thiserror`.
- Serde derives completed on data-model types (`tumult-baseline`,
  `tumult-graph`, `tumult-query`) with round-trip tests.
- Library crates are `tracing`-only — the last `eprintln!` sites
  (`tumult-mcp`, `tumult-lake`, `tumult-ingest`) are converted.
- Doc comments backfilled to full coverage in `tumult-core`,
  `tumult-otel` and `tumult-mcp`.
- CI: the coverage workflow gates PRs at ≥ 90%; the dependabot metadata
  action is SHA-pinned; branch protection on `main` requires green CI
  (Check, Format, Clippy, Test, Security Audit, Cargo Deny, Coverage).
- SECURITY.md refreshed: supported versions, the honest deny.toml
  advisory-exception process, and deployment-security guidance.
- **MSRV raised to 1.92.0** (workspace, CI toolchains, Docker builder
  images, clippy `msrv`, README): `typst` 0.15.1 / `hayro` 0.7 / `krilla`
  0.8 require rustc 1.92, so 1.91.1 can no longer compile the workspace.
  Local, CI and container toolchains are aligned on 1.92.0.

### Fixed
- `tumultd` graceful shutdown no longer hangs: the lake export scheduler
  and the approval-TTL sweeper now honour a cancellation token and drop
  their ingest-writer handles before the drain (both held the writer
  channel open forever).
- The SSH connection pool survives a poisoned mutex instead of failing
  permanently.
- TOON round-trip tests added for `GameDayJournal` and `ResilienceScore`.
- CI: the Security Audit job now runs `cargo deny check advisories`
  instead of `rustsec/audit-check`, which does not read `deny.toml` and
  therefore re-reported advisories already ignored there with documented
  reasons (RUSTSEC-2026-0194, RUSTSEC-2026-0195). One source of truth for
  advisory exceptions.
- CI: the Web UI job runs `svelte-kit sync` before `svelte-check`, so a
  clean checkout (no `.svelte-kit/tsconfig.json` yet) type-checks.

### Security
- `docker-compose.kronika.yml` no longer commits demo credentials in
  production token format — required secrets come from the environment.
- The two `quick-xml` 0.38.4 advisories (RUSTSEC-2026-0194/-0195) remain
  ignored in `deny.toml` with justification: the whole typst → hayagriva
  → citationberg chain is at its latest release with no fix published,
  and the vulnerable reader paths are unreachable (trusted bundled
  templates only). Re-checked every dependency release.

## [2.19.0] — 2026-07-31

**The analytics platform lands in Tumult.** The daemon, OTLP ingestion,
DuckDB lake, compliance reports, query API and SvelteKit UI now ship as
first-class `tumult-*` crates. One store: telemetry, run state, manual
evidence and the journal-analytics family share a single DuckDB
(`~/.tumult/lake.duckdb`, schema v3→v7) behind a single writer, with
`tumult store import-legacy` importing databases from older installs. One
execution path: the CLI's provider
executor is shared (`tumult-exec`), so the daemon runs registered
definitions itself — validated by the same pipeline, e-stoppable,
crash-reconciled. One control plane: server-side auth with route-table
RBAC, and risk-tiered approval workflows with hash-pinned, quorum- and
TTL-bound decisions gate what the daemon dispatches. One surface: the
embedded web UI drives the whole loop from definition registry to
approval queue to compliance-grade R1/R2/R3 reports.

### Added
- **Daemon-run experiments**: tumultd now executes experiments itself, not
  just observes them. `POST /api/runs/validate` runs the CLI's exact
  parse/resolve/validate pipeline as a service and registers the definition
  (SHA-256 content-hash dedup, id `reg-<12 hex>`); `POST /api/runs/dry-run`
  returns the resolved execution plan without executing; `POST /api/runs`
  enqueues onto a bounded in-process queue (`TUMULTD_RUN_CONCURRENCY`,
  `TUMULTD_RUN_QUEUE_DEPTH`; 429 on overload — never silently queued);
  `POST /api/runs/{id}/stop` e-stops: a running experiment's cancellation
  token fires (the runner halts before the next activity and rollbacks
  unwind the fault), a still-queued run is cancelled before it starts;
  `GET /api/runs[/{id}]` expose run state plus the full audit trail. Every
  transition persists through the single-writer channel into schema v4's
  `run_registry` / `runs` / `run_audit`. At startup the daemon reconciles
  runs left active by a previous process lifetime (crash, `kill -9`):
  marked `orphaned`, rollbacks attempted via `run_orphan_rollback`, the
  outcome recorded in the audit trail — a killed daemon no longer leaves
  faults applied. A telemetry loopback exports the daemon's own spans to
  its own gRPC ingest, so daemon-run experiments land in the store and UI
  exactly like CLI runs (ADR-011).
- **Authentication and RBAC for the daemon API** (schema v6, ADR-012): once
  any real user exists, every `/api/*` route requires a session cookie or a
  `kro_`-prefixed bearer token. Browser sessions are opaque 256-bit ids
  stored only as sha256 (`HttpOnly; SameSite=Strict`, 12 h, `Secure` off
  loopback); passwords are argon2id at OWASP parameters; API tokens are
  hashed at rest, individually revocable, and stamp `last_used_at_ns`.
  Authorization is a middleware over a single route table — `viewer <
  operator < approver < admin`, unmatched routes fail closed to admin —
  plus optional per-user environment scopes that filter experiment and run
  visibility. `POST /api/auth/login|logout|change-password`, `GET /api/me`,
  and admin `GET|POST /api/users*` / `POST /api/tokens*` are new; the web
  UI gains a login page (with forced one-time-password change), a 401 →
  /login redirect, and a session user chip. Manual-evidence actors and
  run-audit events record the authenticated username; pre-auth free-text
  actors are attributed to a disabled `legacy` backfill user seeded by the
  migration (it never counts as configured auth, so upgraded stores do not
  lock themselves out). Until the first user exists the API behaves exactly
  as before. The MCP server shares the same `Role` enum from the new
  `tumult-auth` crate and now accepts `approver`/`admin` tokens.
- **Auth bootstrap + bind guard**: `tumultd create-admin` creates the first
  admin with a printed-once one-time password (`must_change` forces
  rotation at first login) while the daemon is stopped. Ported from the MCP
  server: a non-loopback HTTP bind with zero users and no
  `KRONIKA_BOOTSTRAP_ADMIN_PASSWORD` refuses to start; that env var (plus
  `KRONIKA_BOOTSTRAP_TOKEN`) is a loud demo/dev bootstrap path.
- **Authenticated OTLP ingest**: with `KRONIKA_INGEST_TOKEN` set, the
  `/v1/*` HTTP routes and gRPC export methods require
  `Authorization: Bearer <token>` (constant-time compare; `/healthz` stays
  open); an unauthenticated non-loopback ingest bind warns at startup.
  Clients send the token via the standard `OTEL_EXPORTER_OTLP_HEADERS`
  (tumult-otel now attaches it as gRPC metadata), and `tumult run` sends
  `TUMULT_DAEMON_TOKEN` on the journal POST to the daemon.
- **UI execution — the Run page**: the web UI can now drive daemon-run
  experiments end to end. `/runs` lists runs with state badges, timing and
  rollback indicators; `/runs/new` picks a validated definition from the new
  `GET /api/registry[/{id}]` endpoints, renders a parameter form from the
  definition's `${var}` placeholders, previews the resolved plan via
  `POST /api/runs/dry-run`, and starts the run; `/runs/[id]` polls live
  state, shows the audit trail with the authenticated actor, embeds the
  telemetry waterfall as spans land via the OTLP loopback, and carries the
  two-step e-stop (`POST /api/runs/{id}/stop`) with rollback status. Start
  and stop are role-aware (viewer is read-only); `POST /api/runs/validate`
  now records the principal as `registered_by`. The `pending_approval`
  state badge is wired for the approval flow below.
- **Approval workflows and hash pinning** (schema v7, ADR-013): every
  `POST /api/runs` now classifies the resolved definition into a risk tier
  at request time — T0 (pre-approved catalog hash or probe-only) enqueues
  directly; T1 (standard), T2 (staging-class env or destructive-named
  fault) and T3 (production-class env, faults without rollback, or more
  than one fault kind) park in `pending_approval`. Approvals carry a
  canonical pin (SHA-256 over the resolution inputs — definition TOON,
  params, env, target) that is re-verified at dispatch, so edited content
  refuses to run even when fully approved; they expire (T1 72h / T2 24h /
  T3 4h, swept to terminal `expired`) and are single-use. Segregation of
  duties is enforced by the writer (approver ≠ requester, one decision per
  approver). T3 approvals re-run the tumult-autopilot gate in-process
  against current ambient facts (`KRONIKA_AUTOPILOT_POLICY`; fail-closed
  when unset) — a gate Veto can never be approved past. Break-glass
  (admin, mandatory ≥10-char justification) bypasses quorum and TTL but
  never the pin, and opens a retrospective manual-evidence draft as
  compliance debt. `run_audit` events are now hash-chained per run
  (`verify_run_audit_chain` detects tampering) and record the
  authenticated actor. New endpoints: `GET /api/approvals` (viewer),
  `POST /api/runs/{id}/approve|reject` (approver), `POST
  /api/runs/{id}/break-glass` (admin); run detail carries the full
  approval chain. The R2 evidence pack gains an "Approval chain (change
  management)" section (SOC 2 CC8.1), and the web UI gains an `/approvals`
  queue page, the approval chain on run detail, and an admin break-glass
  control. Invalid definitions now fail `POST /api/runs` with 400 at
  request time instead of failing the run at dispatch.
- **tumult-exec**: new crate — the CLI's provider executor
  (`ProviderExecutor` + the native plugin registry) extracted so the daemon
  and the CLI share one execution path.
- **Daemon-first journal ingest**: with `TUMULT_DAEMON_URL` set, `tumult run`
  POSTs the journal to the daemon's new `POST /api/import/journal` endpoint,
  which rides the single-writer channel (`Writer::ingest_journal`) instead of
  losing to the daemon's store lock. Falls back to the direct store write
  only when the daemon gives no HTTP response at all.
- **Lake export + retention for the analytics family**: `experiments`,
  `activity_results` and `load_results` export incrementally on
  `started_at_ns` with the telemetry watermark guard; the `autopilot_*`
  history, `graph_nodes`/`graph_edges` and the `agentic_*` tables export as
  fingerprint-gated full snapshots. Retention purges `autopilot_*` rows only
  while the current fingerprint matches the last export (proving every row
  is in the lake); graph, agentic, manual and audit tables stay exempt.
- **`tumult store import-legacy`**: imports databases from older installs
  (an old `tumult-analytics` store and/or a lake from the earlier analytics
  crates, via `--analytics-db` / `--kronika-db`) into the current store.
  Idempotent natural-key dedupe; older schemas missing later columns import
  via column intersection.
- **tumult-query**: new crate holding the read-side domain queries over the
  unified store (`graph_query`/`graph_neighbors`/`tested_action_names`,
  topology edge/node readbacks + `NodeAttrs`, autopilot status/class-history/
  budget/cooldown reads) as free functions over `&tumult_lake::AnalyticsStore`.
  Writes stay on the store; the TUI and MCP server re-point to `tumult-query`.
- **tumultd daemon + web UI**: the kronikad HTTP daemon is now the `tumultd`
  workspace member, embedding the SvelteKit SPA from `web/` (built with
  `cd web && npm ci && npm run build` — the binary requires `web/build/` at
  compile time).
- **New analytics crates**: `tumult-otlp`, `tumult-lake`, `tumult-ingest`,
  `tumult-metrics`, `tumult-report`, `tumult-compliance`, `tumult-api`
  (renamed from `kronika-*`), plus `tumult-intelligence` gains the
  kronika-ai `llm` and `sql_guard` modules.
- **Demo stack**: `docker/Dockerfile.tumultd` (multi-stage: web build →
  release build → slim runtime) and `docker/docker-compose.kronika.yml`
  (tumultd + experiment-suite seed + report export); the dev collector pair
  moved to `docker/docker-compose.kronika-collector.yml` +
  `docker/otel-collector-kronika.yaml`. The old `kronika-demo` binary and the
  `docker/kronika-legacy-staging/` scaffold are removed.
- **CI**: new `web` job (`npm ci`, `svelte-check`, `npm run build`); check,
  clippy, test and doc jobs now build `web/` before compiling, since tumultd
  embeds the UI.
- **Release**: `release.yml` builds and publishes `tumultd` tarballs
  alongside `tumult-cli` for all targets.

### Changed
- **One DuckDB store**: `tumult-analytics` is replaced by `tumult-lake`
  (schema v3: telemetry + manual evidence + the journal-analytics family —
  experiments, agentic, autopilot, ChaosGraph — in one database file behind
  one writer). The unified store lives at `~/.tumult/lake.duckdb`
  (`TUMULT_LAKE_PATH` override; `TUMULT_ANALYTICS_PATH` and `KRONIKA_DB`
  remain as deprecated aliases for one release). Import existing stores with
  `tumult store import-legacy`.
- Clippy pedantic stays enabled workspace-wide; the new analytics
  crates carry a documented, scoped `#![allow(clippy::pedantic)]` at their
  crate roots (183 pre-existing warnings, intentionally not churned).
- Store schema is now v5: v3 unified telemetry + analytics in one file;
  v4 added the daemon-run tables (`run_registry`, `runs`, `run_audit`);
  v5 rebuilds them without primary keys or secondary indexes — a daemon
  killed mid-write (SIGKILL) can return with DuckDB's ART indexes desynced
  after WAL replay, making every UPDATE fail fatally and poisoning the
  store exactly when orphan reconciliation must write (found by the live
  kill -9 proof). The v4→v5 migration copies data via table scans, which
  never touch the desynced indexes. Orphan reconciliation also changed:
  rollback is now attempted even when the state/audit writes fail — fault
  cleanup is never skipped because the store is degraded.

## [2.18.0] — 2026-07-28

Full OTLP observability: all three signals (traces, metrics, logs) now
export, and every experiment operation is measured and traceable end to end.

### Added
- **OTLP logs pipeline**: `init_logger_provider` builds a batch OTLP/gRPC
  `SdkLoggerProvider`; every tracing event is mirrored to the collector via
  an `OpenTelemetryTracingBridge` layer, stamped with the active trace/span
  ids. `TumultTelemetry::shutdown` flushes and closes it with the other
  providers.
- **Runner metrics wiring**: experiments record `tumult.experiments.total` +
  `tumult.experiment.duration` on every exit (completed, aborted,
  interrupted); actions, probes, and rollbacks record counters + duration
  histograms (`tumult.actions/probes/rollbacks.total`,
  `tumult.action/probe.duration`, `tumult.plugin.errors.total`) tagged with
  plugin and activity name.
- **Sampling spans**: during/post-phase probe sampling now emits
  `resilience.probe` spans parented into the experiment trace tree (the
  detached sampler thread receives the run context explicitly), and samples
  carry real trace/span ids instead of empty placeholders.
- **docker/signoz tooling**: new `Tumult — Operations Logs & Traces`
  dashboard, `import-dashboards.sh` (v2 login, credential-free), and a
  README so anyone cloning can stand up the same observability stack.

### Changed
- Metric names standardized to dot-style (`tumult.experiments.total` etc.)
  to match the shipped SigNoz dashboards; analytics store gauges renamed to
  `resilience.store.*`; dashboard attribute mismatches fixed (`ssh.host`,
  baseline panel query types).
- The MCP server telemetry init now relies on the always-stderr fmt layer —
  no configuration needed to keep the stdio protocol stream clean.

## [2.17.0] — 2026-07-23

A full-platform review (code, docs, blog, website) drove this release:
execution robustness for the fault path, wiring for model features that
parsed but never executed, security hardening of the MCP surface, and a
truthfulness sweep across every published number and example.

### Added
- **Script provider dispatch**: experiments can now execute bundled script
  plugins directly — `provider: { type: script, plugin, function, arguments,
  timeout_s }`. Previously the 11 script plugins were discoverable via
  `tumult discover` but had no run path. `examples/dns-redirect-chaos.toon`
  validates and runs again. Timeout kills the whole process group.
- **`configuration:` / `secrets:` are live**: values resolve before
  templating, substitute as `${config.name}` / `${secrets.group.key}`, and
  inject as `TUMULT_CONFIG_*` / `TUMULT_SECRET_*` env into process and
  script providers (CLI run, GameDay, and MCP paths). Secrets are never
  journaled — covered by a dedicated no-leak test.
- **Declared `controls:` execute**: `cmd_run`, GameDay, and the MCP
  experiment/gameday tools register declared controls at lifecycle events
  (previously an empty registry). Event identity arrives as
  `TUMULT_CONTROL_EVENT` / `TUMULT_CONTROL_ACTIVITY`.
- **OpenTelemetry metrics actually export**: `init_meter_provider` builds a
  real OTLP `SdkMeterProvider`; every counter/gauge/histogram in the
  workspace previously recorded into the noop global meter. The `tumult-mcp`
  binary now initializes telemetry (traces + metrics) with graceful
  degradation and shutdown on all exit paths.
- `tumult export --format arrow` (Arrow IPC; the library exporter existed
  but was unreachable from the CLI).
- `tumult run --force`: the CLI refuses to overwrite an existing journal
  unless forced.
- **MCP HTTP auth**: `Authorization: Bearer` headers are honored via the
  SDK auth middleware (`_meta.authorization` still preferred when both are
  present); `tools/list` requires a valid token when auth is configured;
  per-session rate limiting (`TUMULT_MCP_RATE_LIMIT_RPS` / `_BURST`).
- **Autopilot**: approvals re-run the full gate against current state before
  executing (no stale approvals); a server-wide enactment lock makes the
  `ambient.no_concurrent_experiment` veto real across autopilot and
  experiment/gameday enact paths.
- **Kubernetes**: five functions registered that were implemented but
  undispatchable — `drain_node` (now via the Eviction API, honoring
  PodDisruptionBudgets), `apply_network_policy`, `delete_network_policy`,
  `service_has_endpoints`, `count_pods_in_phase`.
- Rollback scripts: `partition-host-rollback`, `partition-broker-rollback`,
  and `redirect-dns-rollback` are declared in their manifests. Discovery:
  16 plugins, 91 actions.
- Templates: `$${...}` escapes a literal `${...}`; missing-variable errors
  list every missing name.
- CI: a `cargo-deny` job; `check-docs.py` now guards homepage stats,
  cli-reference counts, and blog tool-count claims against staleness.

### Changed
- **Rollbacks run on failure**: the default `on-deviation` strategy now also
  rolls back when a run fails after a fault was injected (previously a
  `Failed` status skipped cleanup on the default path).
- **SIGINT mid-method ends as `Interrupted` with a non-zero exit** — an
  interrupted run can no longer be reported `Completed` (exit 0).
- `tumult analyze` opens the store read-only everywhere, enforces
  SELECT/WITH-only queries, and binds `experiment_id` as a parameter.
- Baselines compute statistics **per probe** instead of pooling
  heterogeneous probes into one bound; sample standard deviation (N−1);
  coefficient of variation uses the absolute mean.
- **Four destructive-annotated MCP tools**: `tumult_run_experiment`,
  `tumult_gameday_run`, and now also `tumult_autopilot_run` /
  `tumult_autopilot_respond` (both can enact fault injection).
- Cloud executors use HTTP clients with connect/request timeouts; AWS
  missing-credential errors no longer mention an instance profile that is
  never queried.
- SSH `command_timeout` is a total deadline (was per-message idle); the
  session pool key includes auth identity and host-key policy.
- Log output moves to stderr, keeping the MCP stdio JSON-RPC stream clean.
- Viewer-role MCP tools ignore `store_path` overrides and always use the
  configured store; autopilot policy parse errors no longer echo file
  content to callers.
- Windows `cpu_stress` clamps absurd worker counts with a warning.
- Plugin discovery tolerates bad paths/manifests per-path and warns on
  shadowing instead of aborting or silently defaulting.
- `parse_duration_str` errors on unparseable input instead of silently
  falling back to 30s/1m/1h.

### Fixed
- **False timeouts on chatty activities**: the CLI, plugin, and MCP process
  executors now drain stdout/stderr concurrently while waiting (bounded,
  with truncation notes) and kill the whole process group on timeout — a
  >64 KiB output no longer deadlocks into a spurious timeout, and
  grandchildren (e.g. `stress-ng`) no longer survive a kill.
- **FaultGate deadlock**: the concurrency slot is an RAII guard (a provider
  panic can no longer wedge gated threads); `max_concurrent_faults: 0` is
  rejected at validation.
- **Foreground provider panics are contained**: the run records the failure,
  writes the journal, and proceeds to rollback instead of unwinding out of
  the CLI. Control handlers are contained at the emit boundary.
- GameDay runs stop the shared load process on every exit path, retain
  completed experiment journals on error, validate experiments, and honor
  SIGINT; `k6` stop is time-bounded.
- Telemetry shutdown runs on all CLI exit paths (spans were lost exactly on
  failed runs); background activities share the run's trace id.
- Analytics ingest is transactional (a mid-ingest failure can no longer
  poison dedup); `AnalyticsStore::default_path` and retention math return
  errors instead of panicking.
- ClickHouse purge reports honest counts (`mutations_sync`) and all ad-hoc
  queries carry timeouts; Cypher clamps row caps and rejects queries over an
  expansion-step budget.
- net-proxy: rollback verifies the pidfile process is really
  `tumult-net-proxyd` before killing (PID-reuse safe), spawn checks
  bind/readiness, and `listen == upstream` loops are rejected.
- Kubernetes in-pod injection validates `iface`/`image` arguments;
  first-pod-match targeting now reports how many pods matched.
- MCP: the process executor kills children on timeout and bounds captured
  output; the concurrency semaphore is acquired after auth; the health
  server bounds connections; agent prompts are passed via stdin, not argv
  (no longer visible in `ps`).
- Script plugins: pumba/netem and loadtest drivers quote and validate
  arguments; kafka `kill-broker` no longer aborts on unset optional
  variables; redis actions validate durations; deleted the undeclared,
  data-destructive kafka `fill-disk` script.
- Kubernetes pod drain honors PodDisruptionBudgets (Eviction API instead of
  direct deletion).
- Docs/truthfulness sweep: `cli-reference.md` matches the 40-tool/30-schema
  server and current flags; QUICKSTART's MCP docker command includes the
  required token; homepage stats bar no longer contradicts itself;
  `docs/index-old.md` (stale homepage with broken install steps) removed
  from the site; topology/autopilot guides appear in navigation; blog posts
  01–05, 07–10, 12, 14, 16 corrected (all embedded experiment examples now
  pass `tumult validate`); testprotocol inventory refreshed.

### Security
- MCP SQL validation rejects DuckDB filesystem/extension table functions
  (`read_text`, `read_csv`, `glob`, …) — a select-only query can no longer
  read arbitrary host files.
- Cloud credentials use `Zeroizing` with redacted `Debug`.
- `docs/security-assessment.md` gained a prompt-injection section covering
  the recommend → generate → run chain and required operator checkpoints.
- `deny.toml` now carries the documented RUSTSEC-2026-0002 (`lru::IterMut`)
  exception the 2.16.1 notes described; Tumult does not call the affected
  API.

### Removed
- `--load jmeter` (it silently ran k6) and the jmeter loadtest drivers.
- `serve.out` runtime log from the repository root.

## [2.16.1] — 2026-07-22

### Changed
- Raised the minimum supported Rust version to 1.91.1 and aligned local,
  container, CI, and release builds on that requirement.
- Reworked the README, guides, and blog index around the current 2.16 feature
  set and replaced unsupported readiness and competitor claims with scoped,
  verifiable language.
- Release publication now requires successful binary builds, both container
  images, and image smoke tests before a GitHub Release is created.

### Fixed
- Restored clean rustfmt, Clippy, rustdoc, and unused-dependency gates on the
  main branch.
- Fixed the 2.16.0 container build failure caused by Rust 1.89 being older than
  the resolved Grafeo dependency's Rust 1.91.1 requirement.
- Corrected stale README counts, the incomplete 40-tool MCP reference, blog
  index gaps, and the 2.16 autopilot gate count.

### Security
- Updated the dependency lockfile to patched releases for the `quick-xml`,
  `quinn-proto`, `crossbeam-epoch`, `anyhow`, and `rand` advisories reported by
  the 2.16.0 security audit.
- Documented temporary `cargo-deny` exceptions for unmaintained transitive
  crates and the `lru::IterMut` advisory; Tumult does not call the affected
  iterator API.
- Removed unused dependencies from the demo control panel and cloud executor.

## [2.16.0] — 2026-07-07

### Added
- **Dynamic guard-telemetry pre-flight**: before enacting, the autopilot
  executes the playbook's guard probe once and evaluates its tolerance — a
  guard that cannot observe the blast downgrades the decision (previously a
  static has-guard check). Addresses the top reported failure mode of
  autonomous chaos: stop conditions bound to dead telemetry.
- **Target enrollment** (structural consent): `require_enrollment` +
  `enrolled_services` in the autopilot policy; un-enrolled targets are
  vetoed by the new hard rule `target.enrolled` (gate is now 14 rules).
- **Change-event triggers**: `tumult autopilot notify-change` (+ MCP
  `tumult_autopilot_notify`, tools now 40) records deploy/config changes in
  an insert-only table; the next pass carries `change_event`-triggered
  revalidation candidates for affected services (change-triggered evidence
  invalidation, not just time-triggered).
- **OTel-derived criticality**: the recommender weighs services by observed
  span rates from `TUMULT_CRITICALITY_FILE` (unit-agnostic relative rates;
  absent data is neutral). The demo extracts real rates from its SigNoz
  ClickHouse in one documented command.
- **Kubernetes service discovery**: `tumult topology discover-k8s` lists
  cluster Services (tier/owner from labels) and emits a *proposed* topology
  TOML for human review — `depends_on` intentionally left empty; discovery
  feeds the reviewed file, never the graph. Unit-tested against a fake
  apiserver (not runnable in the docker demo, by design).
- Demo proof 5 (blind-guard downgrade, enrollment veto, change-event
  decision, criticality-weighted recommendation); release gate: three
  consecutive clean 16-proof suite runs.

## [2.15.0] — 2026-07-07

### Added
- **Autopilot: policy-gated autonomous fault injection.** The deterministic
  recommender proposes, a validator rejects experiments that cannot falsify
  anything, and a 13-rule safety gate (fixed evaluation order, full rule
  trace recorded) decides enact / downgrade / propose / veto. Decisions are
  persisted — with the sha256 of the policy that produced them — *before*
  anything runs. New crate `tumult-autopilot` (pure decision logic + a
  10-scenario replay corpus as gate regression tests).
- **Earned autonomy**: fault classes start propose-only and graduate to
  auto-enact on a clean-run track record (policy thresholds); vetoes and
  failed recoveries reset the ladder. Explicit `[[autopilot.pretrusted]]`
  is the only shortcut.
- **Decision store**: two insert-only DuckDB tables
  (`autopilot_decisions`, `autopilot_events`, schema v4) with no
  update/delete surface, Parquet export (`tumult autopilot export`), and
  ChaosGraph lineage (`recommendation` nodes, `enacted` edges) — "why did
  this run?" is one graph or Cypher query, for vetoed decisions too.
- CLI `tumult autopilot once|status|approve|deny|export`; four new MCP
  tools (35 → 39): `tumult_autopilot_run`, `_status`, `_respond`, `_export`.
- Demo proof 4: pretrusted enact, `ambient.no_open_deviation` vetoes,
  cooldown downgrade, human denial as feedback, lineage + Parquet — all
  asserted. Release gate: three consecutive clean 12-proof suite runs.
- Docs: autopilot guide + blog post with the real gate transcripts.

## [2.14.1] — 2026-07-07

### Fixed
- `resolve_citation` now matches control ids on their alphanumeric skeleton
  (case, whitespace, dots and dashes ignored; parentheses preserved so NIS2
  `Art. 21(2)(b)` and `(c)` stay distinct) and tolerates a redundant
  framework prefix — hand-written ids like `DORA-Art25` or `art25` resolve
  instead of silently producing zero compliance edges.
- Gameday requirement ids canonicalized to citation style (`Art. 24`); the
  committed gameday journal artifact is left untouched as a historical
  record.

## [2.14.0] — 2026-07-07

### Added
- **Service topology & compliance lineage.** Declare your service topology in
  TOML (`tumult topology import`); ChaosGraph gains `depends_on` (service →
  service) and `caused_by` (deviation → fault) relations. The lineage matrix
  computes evidenced / broken / untested per (compliance article, service),
  with conservative fault attribution (failed action → its fault; guard halt
  with a single injected fault → that fault; ambiguity stays unattributed).
- **Topology map** (`tumult topology map`): text, Mermaid and JSON renderings
  with break causes and injection recommendations marked on the map.
- **Injection recommender** (`tumult topology recommend`): deterministic,
  explainable ranking of (service, action, control) — compliance state ×
  citation strength × topology criticality × break proximity × novelty, one
  human-readable reason per factor.
- **Five new MCP tools** (30 → 35): `tumult_topology_import` (Operator),
  `tumult_topology_map`, `tumult_compliance_lineage`,
  `tumult_recommend_injection`, and `tumult_chaosgraph_cypher` — arbitrary
  read-only openCypher over the whole graph via a per-call in-memory engine
  (new `tumult-cypher` crate on GrafeoDB; DuckDB remains the only source of
  truth).
- Deviation nodes now carry halt detail (guard, observed value, safe
  condition) and failing action names.
- Demo: topology document, DORA/NIS2 regulatory mappings on the demo
  experiments, a recommended-run experiment, and `make demo-topology` — three
  scripted proof runs (green lineage / break + attribution / recommend →
  close the gap) captured under `demo/proof/topology/`.

### Changed
- CLI `tumult run` auto-ingest now passes the experiment definition through,
  so CLI runs produce full-fidelity graph rows (services, plugin-keyed
  faults) — previously journal-only.
- Run ingestion merges service-node attrs instead of replacing them, so
  declared topology metadata survives experiment runs.
- `AnalyticsStore::default_path()` honors `TUMULT_ANALYTICS_PATH`.

## [2.13.1] — 2026-07-05

### Changed
- Internal refactor: split the largest modules into cohesive submodules for
  maintainability (MCP tool dispatch by tool family, compliance by concern,
  resource/schema handlers, the demo control-panel). Behavior-preserving — no
  functional change; all tests + clippy green.

## [2.13.0] — 2026-07-05

### Added
- **`tumult tui` (alias `dashboard`) — an interactive analytics TUI** over the
  embedded DuckDB store, opened **read-only** so it coexists with a running MCP
  server / concurrent ingest. Keyboard-driven, tabbed (Experiments / Analytics /
  ChaosGraph / Compliance). The headline is **historical browsing**: the full
  experiment history in chronological order — sortable, filterable, drill into any
  run's activity-duration waterfall + deviations, mark-and-compare runs, and trend
  sparklines (success-rate / duration / resilience over the sequence). A
  **live/paused** auto-refresh surfaces newly-completed experiments in real time.
  New `tumult-tui` crate; ratatui/crossterm. `tumult tui [--store <path>]
  [--refresh-secs <n>]`.

## [2.12.1] - 2026-07-05

Docs: Windows fault injection writeup — blog post ("Windows Chaos, For Real")
and a Windows Faults guide (the 3 tumult-windows faults, the winfault binary,
cross-compilation, and the live Windows 11 guest validation), plus a README
Windows-native faults highlight and blog/guide index entries.

## [2.12.0] - 2026-07-04

Windows-native fault injection — the one fault domain no open-source competitor
offers, and validated live against a real Windows 11 guest (not shipped blind).

Added:
- `tumult-windows` native plugin (5th native executor) with 3 faults:
  - `process_kill` — terminate a process by image name or PID (taskkill /F).
  - `cpu_stress` — saturate CPU with N worker threads for a duration.
  - `network_blackhole` — block a port/host via the Windows firewall (netsh),
    with a rollback that removes the rule.
- Standalone `winfault` binary for in-guest execution; cross-compiles cleanly to
  `x86_64-pc-windows-gnu` (no DuckDB dependency).
- `tumult discover` now lists `tumult-windows` and its 3 functions
  (19 crates, 16 plugins = 11 script + 5 native, 85 actions).

Validated live in a Windows 11 guest (WinBoat/dockur, driven over RDP):
process_kill killed a running notepad and confirmed it gone; cpu_stress drove
guest CPU from ~3% baseline to a sustained ~50% (1 worker on 2 cores) and
recovered; network_blackhole added a firewall block rule, verified it via netsh,
and rolled it back. Command construction is unit-tested on Linux (24 tests).
## [2.11.0] - 2026-07-04

A real, role-aware Web UI — the control panel becomes a deployable product, not a
demo-only page, and it enforces the same RBAC tiers as the server.

Added:
- `tumult_whoami` MCP tool (read-only): returns the caller's resolved role
  (`{role, authenticated}`), so a UI/client can adapt to its permissions. Tool
  count 29 -> 30.
- Web UI app shell: a left-nav layout (Overview / Author / Run / Analytics /
  Compliance / ChaosGraph) replacing the single-scroll demo page; every existing
  card preserved. Theme-aware, responsive.
- Role-aware rendering: the UI calls `tumult_whoami` on load and shows a role
  badge; a **viewer** gets the Run/inject section locked (read-only banner,
  disabled operator controls) while keeping Author-preview/Analytics/Compliance/
  ChaosGraph; an **operator** gets everything. Defense in depth — the JS gates
  actions and the server enforces regardless. Whoami failure assumes least
  privilege (viewer).
- Product decoupling: neutral `TUMULT_UI_*` env vars (with `DEMO_*`/legacy
  fallbacks so the demo is unchanged) let the same UI run against any tumult-mcp;
  `demo/control-panel/README.md` documents standalone deployment (env table,
  viewer/operator behavior, Compose + k8s snippets).
## [2.10.0] - 2026-07-04

Role-based access control on the MCP server — the safety control that answers
"who may fire chaos in production", and the natural follow-on to 2.9's
secure-by-default work.

Added:
- **2-role RBAC (viewer / operator)** on the MCP server. `viewer` may call
  read-only tools (analyze, query, chaosgraph, compliance, discover, catalog,
  scaffold, …); `operator` may call everything, including fault injection,
  container kill/pause, and running experiments. Fail-closed (default-deny):
  an unknown or unmapped token is rejected, never elevated.
- Token → role mapping from a TOML auth config (`--auth-config <path>` /
  `TUMULT_MCP_AUTH_CONFIG`, default `~/.tumult/mcp-auth.toml`):
  `[[tokens]] token = "…" role = "viewer"|"operator"`. Tokens are compared in
  constant time. A malformed config aborts startup rather than running open.
- Tools are classified by their `read_only_hint`; a test cross-checks the role
  table against every tool's hint so the two can never silently diverge
  (23 viewer, 6 operator).

Changed:
- Backward-compatible: with no config file, the existing single
  `TUMULT_MCP_TOKEN` maps to `operator`, so current deployments are unchanged.
- The secure-by-default bind guard now refuses a non-loopback HTTP bind unless
  auth is configured (a config file OR a token).
- Docs (production-deployment §Security, README) describe the 2-role model,
  config format, priority order, and rotation.
## [2.9.0] - 2026-07-04

Authoring ergonomics + production-readiness: pick a fault and get a validated
experiment in seconds, and run the server safely in production.

Added:
- `tumult-authoring` crate: a fault catalog derived live from the plugin set (so
  it never drifts from the real actions), an experiment builder that emits
  validated TOON, and 10 curated starter templates.
- CLI: interactive `tumult new` (domain -> fault -> args -> target -> probe ->
  validated .toon), `tumult new --from <template> [--set k=v]`, `tumult templates`.
- MCP: `tumult_fault_catalog` + `tumult_scaffold_experiment` tools (both read-only);
  the demo control panel gains a "New experiment" fault-picker card driving them.
- Production deployment: `deploy/systemd/tumult-mcp.service` (hardened unit),
  `deploy/k8s/tumult-mcp.yaml` (Deployment/Service/PVC/Secret), and
  `docs/guides/production-deployment.md` (security, TLS, store model, backup, BYO
  collector, blast-radius).

Security (production):
- Secure-by-default MCP serve: binds `127.0.0.1` by default and REFUSES to serve
  HTTP on a non-loopback address without `TUMULT_MCP_TOKEN`. The shipped image no
  longer exposes tools unauthenticated on 0.0.0.0. Replaced the dead
  `mcp_bind_address` with an enforced `host_is_loopback` guard. The demo passes
  `--host 0.0.0.0` explicitly (it sets a token).

Changed:
- `chaosgraph_coverage_gaps` derives read-only by default (coexists with a running
  server); persisting the gap sub-graph is opt-in via `--refresh` (CLI) and never
  happens from the MCP tool.
- Documented `blast_radius` (advisory) vs `max_concurrent_faults` (enforced).
## [2.8.0] - 2026-07-04

A cohesion release: close the gaps between what Tumult ships and what a user can
actually reach and trust. Driven by an SRE-usability + journey-cohesion audit.

Added:
- `tumult mcp serve [--transport stdio|http] [--port] [--token]` — the MCP server
  is now reachable from the main binary (refactored into a reusable `server`
  module); it was previously a separate, undiscoverable executable.
- `tumult chaosgraph query|neighbors|coverage-gaps` — ChaosGraph is now usable by
  a human from the CLI, not only by agents over MCP. Reads open the store
  read-only so they coexist with a running server.
- Demo: the flagship `make demo` control panel now surfaces the whole journey —
  Analytics, DORA Compliance, and ChaosGraph cards (driving existing MCP tools),
  plus a Safety-guardrail card showing an auto-halt run. The compliance/analytics
  payoff no longer lives only in a separate stack.
- `AnalyticsStore::open_read_only` — read-only opens that coexist with the writer;
  `AnalyticsError::StoreLocked` maps the opaque DuckDB lock error to a clear,
  actionable message.

Changed:
- MCP read tools (chaosgraph query/neighbors, analyze_store, recommend) now open
  the store read-only, so the CLI and the running server no longer collide on the
  single-writer DuckDB store.
- CLI is quiet by default: telemetry INFO lines are suppressed unless an OTLP
  endpoint (or RUST_LOG) is set.
- `tumult analyze <dir>` skips experiment `.toon` files quietly instead of warning;
  empty store prints a clean message instead of `avg=NULLms`.
- Corrected stale counts across README (17 crates, 15 plugins, 82 actions, 4
  native) to match `tumult discover`.
- The 3 previously-unwired demo experiments (guard-halt, timewarp clock/entropy)
  are now driven from the demo.

Fixed:
- First-run: `install.sh` verification scaffolds a self-contained experiment via
  `tumult init` instead of running a non-existent file, so a fresh clone verifies
  cleanly.
- `tumult init` help/wording no longer claims to be interactive (it scaffolds a
  template).
- Demo SigNoz trace link points at a page that exists with an explicit filter
  hint, instead of a guessed query that landed on an unfiltered view.
## [2.7.2] - 2026-07-04

Added:
- `demo/proof/validate.py` + `make demo-proof`: a suite that validates Tumult's
  headline claims against the LIVE demo (no mocks) — ChaosGraph token efficiency,
  the first-class MCP surface (27 tools, annotations, outputSchema, auth,
  isError), and agentic trajectory contracts. Thresholds are set from measured
  behaviour; the suite exits non-zero on any failure. 15/15 checks pass.

Changed:
- ChaosGraph `neighbors` with a `rel` filter now follows ONLY that relation, so a
  targeted structural query returns just the reachable nodes (e.g. the fault),
  not the whole accumulating neighbourhood. This makes a targeted answer bounded
  (O(1)) regardless of run history.
- Corrected the ChaosGraph token claim across README/docs/blog: replaced the flat
  "~37x" (which was benchmarked on a large GameDay journal + a fresh graph) with
  the reproducible framing — a targeted answer stays ~110 tokens no
  matter how many runs accumulate, the graph is ~8x more compact per run of
  history, and store-wide questions cost ~20x less than reading every journal.
  Every figure is reproducible via `make demo-proof`.
## [2.7.1] - 2026-07-04

Fixed:
- tumult-ssh: `append_known_hosts_entry` wrote the trust-on-first-use entry but
  never flushed. `tokio::fs::File` buffers internally and does not flush on drop,
  so the just-written host key could still be in-buffer when a subsequent read
  ran — a race that surfaced as an intermittent test failure under load and could,
  in production, make a follow-up verify miss a just-recorded key. Now flushed
  before returning.

## [2.7.0] - 2026-07-04

Added:
- ChaosGraph Phase 2 — the graph now answers coverage and compliance questions,
  not just "what did this experiment touch":
  - ComplianceArticle nodes (one per citation in the compliance registry) with
    strength-weighted `evidences` / `maps_to_compliance` edges from experiments
    that declare a regulatory mapping.
  - CoverageGap + FaultDomain nodes: actions in the plugin catalog never seen in
    a tested run, grouped by plugin.
  - New MCP tool `tumult_chaosgraph_coverage_gaps {framework?, domain?}` (27
    tools total) — returns untested actions, optionally with the compliance
    articles a framework still leaves unevidenced.
  - Process-provider service extraction (the Phase-1 gap): `docker exec/pause/...
    <container>`, a curl URL host, or an ssh host now yield a Service node +
    `targets` edge — conservatively, only when confidently extractable. Validated
    live: demo-postgres now produces `svc:demo-postgres`; coverage-gaps returns
    52 untested actions; 16 compliance-article nodes seeded.
  - Analytics store schema v2 -> v3 (additive: graph edge attrs + compliance seed).

Fixed:
- Dockerfile.tumult copied workspace crates via a hand-maintained per-crate list,
  which silently dropped new crates (tumult-graph, tumult-cloud) and failed the
  GHCR image build while release binaries succeeded. Now `COPY . .` with a
  tightened .dockerignore, so adding a crate can never break the image again.
## [2.6.0] - 2026-07-04

Added:
- Deepened agentic resilience testing (the differentiator no other chaos tool
  has): multi-turn agent-graph fault modeling. Model an ordered model+tool
  trajectory and inject a fault at a specific step, with retrieval context
  propagating forward so grounding failures cascade like a real RAG agent.
  Whole-trajectory contracts (recovers-within, no-repeated-step,
  terminates-healthy, step-budget) and per-dimension agentic subscores
  (recovery, cost-control, correctness-under-fault, loop-avoidance). Three
  trajectory packs — rag-grounding-failure, reflection-loop, multi-tool-cascade
  — with meaningful pass/fail cases. New `tumult agentic trajectory --pack`;
  metadata surfaced in the MCP list-scenarios tool. Validated on the live demo
  (demo-agentic-trajectory, in the standard sweep).
- Cloud fault connectors (`tumult-cloud`): thin connectors to providers' own
  fault services — AWS FIS (start/stop/status experiments) + direct EC2
  stop/terminate, Azure Chaos Studio (start/cancel/status), and GCP Compute
  instance-stop (GCP has no managed chaos service — documented, not faked).
  A hand-rolled SigV4 signer (pinned to the AWS get-vanilla test vector) instead
  of the heavy AWS SDK, preserving the single-binary ethos. Credentials from the
  standard provider chains, fail-fast, never logged. Validated by 36 hermetic
  mocked-HTTP tests; real-cloud paths documented (exempt from the docker demo).
## [2.5.0] - 2026-07-04

Added:
- In-pod Kubernetes data-plane faults, without a privileged DaemonSet — injected
  via the ephemeral-containers subresource (the `kubectl debug` mechanism):
  - `pod_network_latency`: attaches an ephemeral container that runs
    `tc qdisc netem delay` in the target pod's shared network namespace,
    self-terminating after `duration_s`.
  - `pod_stress`: runs `stress-ng` (CPU or memory) in the target container's
    process namespace, self-terminating via `--timeout`.
  Closes the biggest capability gap vs Chaos Mesh/Litmus (control-plane-only k8s)
  while preserving the no-control-plane, single-binary identity. Limits
  documented (ephemeral containers GA since k8s 1.25, cannot be removed once
  attached, `tc` needs NET_ADMIN). Validated by hermetic fake-apiserver tests
  asserting exact apiserver traffic + a k3d validation script (scripts/k8s-demo.sh)
  and examples/k8s-pod-{latency,stress}.toon.
## [2.4.0] - 2026-07-04

Added:
- ChaosGraph (MVP): a typed knowledge graph over chaos data, built from journals
  as they ingest and served to agents over MCP for token-efficient context. New
  `tumult-graph` crate (pure model) + DuckDB `graph_nodes`/`graph_edges` tables
  (analytics schema v2, additive migration). Two read-only MCP tools —
  `tumult_chaosgraph_query` and `tumult_chaosgraph_neighbors` (24 -> 26 tools) —
  return compact sub-graphs instead of whole journals (~37x fewer tokens for
  "what did this experiment touch?"). Each run appends one journal node to the
  experiment's neighbourhood.
- Time & entropy fault family (`tumult-timewarp` script plugin, 14th plugin):
  clock skew (per-process libfaketime), clock-driven auth failure
  (cert-past-expiry, token-TTL), entropy drain, and RNG/crypto pressure, with
  probes for entropy-available / crypto-throughput / clock-offset. Two demo
  experiments prove it end to end.

Fixed:
- Dockerfile.tumult built `tumult-net-proxyd` with a `--bin` scoped to the wrong
  packages, breaking the GHCR image publish and a fresh `make demo` image build.
  Now built via `-p tumult-net`. (Release binaries were unaffected.)
## [2.3.0] - 2026-07-04

Added:
- Auto-halt guardrails: an experiment may declare `guards` — probes evaluated
  continuously during the fault window whose tolerance describes the SAFE
  condition. On breach the runner cancels the method, runs rollbacks, and marks
  the run `Halted` (a new experiment status) with a halt record (guard name,
  observed value, time-to-halt). Optional `blast_radius` note and
  `max_concurrent_faults` cap, surfaced in the journal. No guards = pre-2.3
  behaviour exactly. Exposed through the CLI, the MCP run tool, and the demo
  (`demo-guard-halt.toon` proves it end to end).
- Demo control panel: a "Run the whole chaos loop via MCP" showcase that drives
  discover → validate → run → analyze → recommend as pure MCP tool calls.

Changed:
- Compliance mappings audited against official sources and corrected. Every
  citation now lives in a single dated, sourced registry (framework, control id,
  evidence type, evidence strength, source URL, last_verified) shared by the CLI
  and MCP. `tumult compliance --sources` lists them; a staleness test fails once
  any citation exceeds 18 months unverified. Wrong/outdated citations fixed
  (e.g. ISO 27001 A.17 → A.5.30; PCI pen-testing overreach removed; DORA Art. 28
  retention claim corrected). The verdict is reframed as evidence toward a
  control, explicitly NOT a compliance determination.

## [2.2.0] - 2026-07-04

Added:
- One-command demo (`make demo`): a single Docker network with an OTel-instrumented
  axum+Postgres app, SigNoz observability with pre-imported dashboards, the Tumult
  CLI+MCP server, per-domain fault injection (net, postgres, container, stress,
  process, ssh, agentic), a continuous traffic generator, and a web control panel
  that drives faults as an MCP client. `make demo-check` runs the same sweep
  headlessly as a functional smoke test (exit non-zero on failure).
- `tumult-net-proxyd` is now packaged in the container image alongside the main
  binary, so tumult-net userspace-proxy faults work out of the box.

Fixed:
- Regex tolerance now matches against a probe's serialized JSON value, so a
  pattern like `ok` matches a `{"status":"ok"}` health endpoint (previously only
  bare JSON strings matched, silently aborting experiments).
- `sync_await` no longer panics when a native plugin (ssh/net/kubernetes) executes
  on the runner's scoped worker thread — it falls back to a temporary runtime when
  no ambient one exists. Native plugins previously crashed the process in the real
  experiment runner (masked by tests that ran inside a Tokio runtime).
- tumult-net `upstream`/`listen` now resolve DNS hostnames (`demo-app:8080`), not
  just literal IPs — the norm on container/Kubernetes networks.

Changed:
- Release profile switched from fat to thin LTO (codegen-units 16): near-identical
  runtime performance for this IO-bound CLI, far faster and more reliable builds
  (fat LTO could intermittently SIGSEGV rustc on a workspace this size).

## [2.1.0] - 2026-07-04

The MCP server grows from 19 to 24 tools and becomes a spec-faithful,
first-class operator surface: tool annotations on every tool, structured
content with advertised output schemas on 16, workspace files served as
`tumult://` resources, and a run→ingest→recommend feedback loop that now
closes entirely over MCP.

### Added

- **Five new MCP tools** (19 → 24):
  - **`tumult_report`**: render a journal as `json` (raw journal) or
    `junit` XML via the shared `tumult_core::report` renderers (extracted
    from the CLI). With `output_path` the report is written inside the
    workspace; otherwise the content is returned inline, capped at
    512 KiB. HTML/PDF remain CLI-only.
  - **`tumult_compliance`**: pass rate, recovery compliance, and
    COMPLIANT/PARTIAL/NON-COMPLIANT verdict over a journal file or
    directory for one of seven frameworks (`dora`, `nis2`, `pci-dss`,
    `iso-22301`, `iso-27001`, `soc2`, `basel-iii`). The scoring and
    framework catalog moved into the new `tumult_core::compliance` module
    so `tumult compliance` and the MCP tool share one source of truth.
  - **`tumult_trend`**: cross-run metric trend over journals
    (`resilience_score`, `duration_ms`, `estimate_accuracy`,
    `method_step_count`; optional `last` window and `target` title filter)
    with time-ordered `{ts, value}` points and a direction verdict.
  - **`tumult_gameday_create`**: scaffold a `.gameday.toon` campaign
    (experiments, optional k6/jmeter load config, compliance framework)
    via the shared `tumult_core::types::gameday_toon_template` (also now
    used by `tumult gameday create`). Unlike the CLI it refuses to
    overwrite an existing file and requires `load_script` when a load tool
    is chosen.
  - **`tumult_agents`**: agent CLI adapter detection table (`claude-code`,
    `codex` — installed/version/auth state). Probes local binaries by
    spawning short version checks; documented in the tool description.
- **`tumult_recommend` agent parameters**: `agent`, `agent_model`,
  `agent_timeout_secs`, `generate_experiments_dir` bring the CLI's 2.0
  agent-enhancement flow to MCP. Generated experiments pass the same
  parse+validate gate as the CLI (now shared as
  `tumult_intelligence::write_validated_experiments`). The tool is
  annotated `open_world_hint=true` / `read_only_hint=false` because the
  agent CLI may reach its model API and validated experiments are written
  to disk.
- **Tool annotations on all 24 tools**: every tool now declares
  `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`,
  so MCP clients can auto-approve reads and gate chaos — 18 tools are
  read-only and idempotent, 2 are destructive and open-world
  (`tumult_run_experiment`, `tumult_gameday_run`), and 4 are
  non-destructive writers (`tumult_create_experiment`,
  `tumult_gameday_create`, `tumult_report`, `tumult_recommend`).
- **Structured content + output schemas on 16 tools**: results carry
  `structuredContent` alongside the existing text, and `tools/list`
  advertises a matching `outputSchema` (hand-written compact JSON Schemas
  derived from the serde types, journal status/activity enums included) so
  clients validate results instead of parsing prose.
- **MCP resources**: the server now declares the `resources` capability (no
  subscriptions or list-changed notifications yet) and serves workspace
  files under three URI schemes — `tumult://journal/{filename}` (journals,
  read as the same `{summary, journal}` JSON as `tumult_read_journal`, with
  the 512 KiB cap degrading to the summary plus a note),
  `tumult://experiment/{filename}` and `tumult://gameday/{filename}` (raw
  TOON text, `application/toon`). Filenames only — path separators and
  traversal are rejected through the same containment helpers as tools, and
  resource requests pass the same `_meta.authorization` bearer gate.
- **`resources/list` pagination**: cursor-based pages of 100 (opaque base64
  offset cursors; invalid cursors are protocol errors) over the sorted flat
  listing of `.toon` files in the workspace root.
- **`resource_link` content items**: `tumult_run_experiment` links the
  written journal, `tumult_gameday_create` the created campaign file,
  `tumult_report` (with `output_path`) the written report, and
  `tumult_list_journals` one link per listed journal (capped at the first
  50). Text content is unchanged and remains the first content block.
- **List tool pagination**: `tumult_list_journals`,
  `tumult_list_experiments`, and `tumult_gameday_list` accept optional
  `limit` (default 100, max 1000) and `offset` parameters, sort their
  results, and now return structured content `{items, total, offset,
  limit}` (with advertised output schemas) alongside the existing text line
  formats.

### Changed

- **`tumult_run_experiment` closes the MCP feedback loop.** It now persists
  the journal (`journal_path`, default `journal.toon` — CLI parity) and
  auto-ingests it into the analytics store, so `tumult_recommend`,
  `tumult_coverage`, and `tumult_trend` see MCP-driven runs without a CLI
  round-trip. New parameters `journal_path`, `no_ingest`, `store_path`, and
  `format`; the result reports the ingestion outcome (`ingested` /
  `duplicate` / `skipped` / `failed: <reason>` — ingestion failures are
  warnings, not run failures). Previously the tool returned the journal
  text and discarded it.
- **Journals over MCP are JSON by default**: `tumult_run_experiment` and
  `tumult_read_journal` return the journal as JSON (`format=toon` for the
  raw TOON text), and `tumult_read_journal` gained `summary=true` for a
  compact summary instead of the full journal.
- **Strict enum parameters**: `format`, `rollback_strategy`, `framework`,
  `metric`, and `load_tool` now reject unknown values with an error listing
  the valid ones, instead of silently defaulting.
- **512 KiB text cap**: all inline tool text content is capped at 512 KiB
  with an explicit truncation notice appended.
- **`tumult_recommend` now runs on `tumult-intelligence`** instead of a
  parallel DuckDB re-implementation, so MCP and CLI recommendations cannot
  drift. Its structured output now mirrors the serialized
  `RecommendationOutput` (`source`, `recommendations`, `draft_toon`,
  `notes`, `heuristic_context`, optional `agent`) and the advertised output
  schema was updated accordingly.
- **Shared logic extracted to library crates** so the CLI and MCP server
  render from one implementation: `tumult_core::compliance` (framework
  scoring), `tumult_core::report` (JSON/JUnit renderers),
  `tumult_core::runner::k6` (k6 load executor, moved from `tumult-cli`,
  which re-exports it unchanged),
  `tumult_core::types::gameday_toon_template`, and
  `tumult_intelligence::write` (validated experiment writing).
- **Test suite growth**: 921 tests across the workspace (up from 876).

### Fixed

- **`tumult_gameday_run` executes declared shared load.** It previously ran
  with `RunConfig::default()` (no load executor), silently hollowing the
  load-impact component of the resilience score. It now wires the same k6
  executor as `tumult gameday run`, and reports `Load: declared but
  produced no result` when the load tool fails to start instead of omitting
  it silently.

## [2.0.0] - 2026-07-04

### Breaking

- **`http` provider removed** from the experiment format. It was never
  implemented — every `type: http` activity errored at runtime — so the
  variant is gone from `Provider`, and `type: http` in a `.toon` now fails
  validation with an unknown-variant error. Use `type: process` (e.g. `curl`)
  or a plugin action instead.
- **SSH native `execute` now verifies host keys by default.** The hardcoded
  accept-any host key check is gone. A new `host_key_policy` argument selects
  `verify` (default — checks `known_hosts`), `trust-on-first-use`, or
  `accept-any` (explicit opt-in for ephemeral targets). `SshSession::connect`
  surfaces typed `HostKeyNotFound` / `HostKeyMismatch` errors.
- **GameDay experiment/journal count mismatch is a hard error.**
  `run_gameday` now returns `RunnerError::ExperimentCountMismatch` when the
  number of provided experiments doesn't match `gameday.experiments`, instead
  of a debug-only assertion. `compute_compliance_coverage` is now internal to
  the gameday runner and operates on zipped
  `(&GameDayExperiment, &Journal)` pairs.
- **`tumult-analytics` gained a `duckdb` cargo feature** (enabled by
  default). Consumers that don't need the embedded DuckDB engine — like the
  ClickHouse backend, which now depends on `tumult-analytics` with
  `default-features = false` — no longer compile DuckDB.

### Added

- **`tumult-agent-cli` crate**: an adapter layer for invoking agentic coding
  CLIs (Claude Code, OpenAI Codex) non-interactively — one-shot batch
  subprocess calls with no TTY and no session persistence. Provides the
  `AgentCliAdapter` trait (detect / build / run / parse / explain),
  `AdapterRegistry::builtin()`, and `run_prompt` with typed `AgentCliError`s.
  Binary resolution honors `CLAUDE_CODE_BIN` / `CODEX_BIN` env overrides
  with a `PATH` fallback; the workspace is now 15 crates.
- **`tumult recommend --agent <name>`**: enhance the deterministic
  recommendations with a local agent CLI (`--agent-model`,
  `--agent-timeout`, `--generate-experiments <dir>`). The agent receives the
  heuristic report, journal signals, and the plugin catalog in one
  self-contained prompt and returns re-ranked recommendations plus optional
  `.toon` experiment proposals. Every proposal passes a validation gate
  (`parse_experiment` + `validate_experiment`) before being written to
  `<dir>/<title-slug>.toon` (no overwrites — `-2`, `-3`, ... on collision);
  invalid proposals are rejected with the error and counted in the summary.
  JSON output gains an `agent` object with `experiments_written` /
  `experiments_rejected`. New `tumult_intelligence::agent` module
  (`build_agent_prompt`, `enhance`, `split_toon_blocks`).
- **`tumult agents` command**: table of detected agent CLI adapters — name,
  installed, version, auth detail, and an install hint when missing.
- **Real probe sampling and recovery measurement** in `tumult-core`:
  during-phase hypothesis probes are sampled on a real interval (default 1s,
  capped at 300 samples) concurrently with fault execution, and post-phase
  sampling loops until the probes pass tolerance again or a 30s recovery
  timeout expires — so `recovery_time_s` / `mttr_s` measure observed
  recovery. New public `SamplingConfig { interval, max_during_samples,
  recovery_timeout }` and `run_experiment_with_sampling`. The journal
  records the actual sample interval used. Experiments without hypothesis
  probes skip sampling; already-healthy probes finish the post phase in a
  single round.
- **Native plugin architecture**: new `NativeExecutor` trait and
  `NativeExecutorRegistry` in `tumult-plugin` (`src/native.rs`), with
  implementations living in their own crates (`tumult-ssh`, `tumult-net`,
  `tumult-kubernetes` — `src/native.rs` each). `tumult-cli` is a pure
  composition root that registers the executors. Unknown plugin or function
  names now error with the list of available names.
- **Native plugins in discovery**: `tumult discover` and the MCP
  `tumult_discover` tool now list native plugins alongside script plugins —
  13 plugins / 64 actions in total — labeled `(script)` / `(native)`, with
  a sorted, counted action list. `tumult discover --plugin` also accepts
  native plugin names (e.g. `tumult-ssh`) and shows their functions. The
  MCP server registers the same three executors at its own composition
  root (`tumult-mcp/src/native.rs`), mirroring the CLI's, and
  `NativeExecutorRegistry` gained a `qualified_functions()` helper both
  binaries render from.
- **`cargo machete`** added to the CI lint job to keep unused dependencies
  out of the workspace.

### Changed

- **MCP error semantics**: tool failures now set `isError: true` on the
  result per the MCP specification, and authentication / rate-limit
  rejections are reported as such instead of as "Unknown tool".
- **Module layout cleanups** across crates and pruning of unused
  dependencies (enforced by `cargo machete`).
- **Test suite growth**: 834 tests across the workspace (up from 755 before
  this round).

### Fixed

- **`tumult_create_experiment` now actually works over MCP** — the tool
  previously failed when invoked through the server.

### Fixed

- **`tumult-net` probe test**: `reachable_is_false_for_a_closed_port` no longer
  flakes under fully-parallel test runs. It retries with fresh ephemeral ports so
  a sibling test transiently re-binding a just-freed port can no longer fail it.
- **HTML report**: dropped the fabricated `tumult replay --trace <id>` hint on
  failed activities. The core experiment path has no per-activity diagnostic
  command (`next_diagnostic_command` exists only in agentic result types) and
  `activity_results` is not keyed by trace, so the suggestion was misleading
  (`tumult replay` takes a fixture path, not a trace). The captured error text
  and the trace column remain as the actionable signal.

## [1.5.0] — 2026-07-03 — Privilege-free network chaos + CI-ready reporting

### Added

- **`tumult-net`**: a new native crate providing a userspace TCP chaos proxy
  built on [`tokio-netem`](https://crates.io/crates/tokio-netem) `0.1.1` (MIT).
  It forwards TCP traffic through a detached `tumult-net-proxyd` daemon and
  injects directional faults with no `tc`/`iptables`/`NET_ADMIN` privileges:
  `inject_latency`, `throttle_bandwidth`, `fragment_stream`, `corrupt_bytes`,
  `terminate_connections`, and the composite `start_proxy`, all rolled back by
  `stop_proxy`. Read-only probes `reachable` and `measured_latency` validate
  steady state. A `seed` makes the fault schedule (jitter) and the byte
  corruption / termination RNGs reproducible. Wired into the CLI native dispatch
  as plugin `tumult-net`; new example `examples/net-chaos.toon`. Spans use the
  `net.*` domain prefix and attach under the resilience parent spans.
- **`tumult report --format junit|json`**: machine-readable report output for CI
  gating (one JUnit `<testcase>` per activity; JSON for tooling), alongside the
  existing HTML.
- **HTML report**: optional clickable trace links (`--trace-ui-base` /
  `TUMULT_TRACE_UI_BASE`, off by default, using the full trace ID), per-activity
  failure detail and `next_diagnostic_command` hints, and a footer recording the
  generation timestamp, tool version, and journal content hash for audit
  defensibility.

### Changed

- **Compliance verdict** (`tumult compliance`) now requires both a passing
  pass-rate and a recovery signal (MTTR under target, or `resilience_score`,
  falling back to pass-rate-only with an explicit reduced-assurance warning),
  instead of raw completion rate — closing a false-`COMPLIANT` gap.
- **Internal refactor**: every Rust source file over 400 lines was split into
  `mod.rs` submodules (public APIs preserved via re-exports); no behavior change.

## [1.4.0] — Code review hardening pass

### Added

- **k6 JSON summary export**: `tumult-cli`'s k6 load executor now runs k6 with
  `--summary-export` and parses the stable JSON summary first, falling back to
  the human-readable text output only if the file is missing or unparseable.
  Metrics that fail to parse from either source now log a `tracing::warn!`
  instead of silently defaulting to `0`.

### Changed

- **Runner phase ordering**: during-method steady-state probe sampling now runs
  concurrently with the method (on a background thread) instead of after
  rollback, and post-method recovery sampling now runs immediately after the
  method completes rather than at the very end of the experiment. This makes
  `during_result` and `post_result.recovery_time_s` reflect the fault window and
  recovery from the fault itself, rather than recovery from rollback actions.
- **`tumult-cli`**: `commands.rs` (4,155 lines) split into a `commands/` module
  (`exec`, `load`, `run`, `store`, `report`, `gameday`, plus `mod.rs` and
  `tests.rs`) along its existing section boundaries; no behavior or public API
  changes. The `arg_u32`/`arg_i32`/`arg_u16` helpers were consolidated into one
  generic `arg_num::<T>`.
- **`tumult-core`**: deduplicated rollback execution, journal construction, and
  hypothesis-evaluation activity-span boilerplate in `runner.rs` via a shared
  `execute_single_activity` helper and `Journal::for_experiment` constructor.
- **GameDay compliance coverage**: `compute_compliance_coverage` now zips
  controls with their evaluations instead of indexing by position, avoiding
  silent misalignment if the two lists ever diverge.
- **Template variable substitution**: `${var}` values are now escaped for TOON
  string context (quotes, backslashes, newlines) before being spliced into the
  encoded experiment, preventing a variable value from breaking the document or
  injecting structure.

### Fixed

- **Background activity panics**: a panicking background activity now produces
  a `Failed` result carrying that activity's own name and type, instead of being
  misattributed to a different activity by join order.
- **SSH command logging**: `tumult-ssh`'s instrumented `execute()` no longer
  panics when truncating a command for tracing if the truncation point falls
  inside a multi-byte UTF-8 character.
- **Sync process execution timeout**: `execute_process_sync` now polls the
  child process and kills it on timeout instead of leaking it in the background
  via a detached thread.
- **MCP SQL guard**: `validate_select_only` hardening — rejects stacked
  statements and forbidden keywords as standalone tokens (not substrings),
  closing gaps in the read-only query guard.

## [1.3.0] — Cross-client agentic observability

### Added

- **Canonical agentic telemetry schema (`tumult-otel::agentic`)**: a single home
  for the agentic observability vocabulary — `resilience.agent.*` and `gen_ai.*`
  attributes, the `tumult.client` resource tag (`claude-code` / `codex` /
  `copilot` / `opencode` / `unknown`), `GenAiOperation`, and `TelemetryEvidence`,
  migrated out of `tumult-agentic` so observability lives in one place.
- **W3C trace-context helpers (`tumult-otel::propagation`)**: `parse_traceparent`,
  `inject_traceparent`, and `current_traceparent` used by both the proxy and MCP
  surfaces.
- **Experiment-side instrumentation**: every agentic run now emits a
  `resilience.agentic.experiment` span (with a child event per fault decision and
  per contract outcome) plus run metrics, via `tumult-otel::agentic_span` — for
  offline scenario-pack and replay runs too, not just live targets.
- **Proxy trace propagation**: each proxied request gets a `tumult.agentic.fault`
  span parented under the client's inbound `traceparent` (standalone + `tumult.client`
  tag when absent), and propagates its context upstream.
- **MCP tool-surface span**: agentic MCP tool calls are wrapped in a
  `tumult.agentic.tool` span the experiment span nests under (correlate tier; the
  MCP transport hides the inbound `traceparent`).
- **Client profiles + `--client` selector**: declarative per-client wiring
  (base-URL env, native-OTel, per-surface trace-nesting tier) for the four clients.
- **Orchestrator mode (`tumult agentic run-live`)**: tumult as the trace root —
  starts a `tumult.experiment` span, runs `claude -p` with the minted trace
  context + telemetry + proxy base URL, and evaluates contracts against the
  agent's response. The agent call is behind an `AgentRunner` trait (testable).
- **Collector normalization**: `collector/otel-agentic-normalization.yaml` plus
  the same OTTL transform grafted into the docker lab collector, normalizing the
  four clients' native telemetry onto the canonical schema.
- **Docs**: cross-client observability guide.

### Fixed

- `retrieval_poisoning` now contaminates the evaluated response body (regression
  noted in 1.2.0 follow-up work) and `tumult-agentic` no longer defines its own
  OTel attribute constants (now sourced from `tumult-otel`).

## [1.2.2] — Dependency upgrades + CI action bumps

### Changed

- **Dependency upgrades** (all verified green with `cargo build`/`test`/`clippy`
  across the workspace): OpenTelemetry stack `0.31` → `0.32`
  (`opentelemetry`, `-sdk`, `-otlp`, `-semantic-conventions`, `-stdout`,
  `-appender-tracing`) and `tracing-opentelemetry` `0.32` → `0.33`; `toon-format`
  `0.4` → `0.5`; `duckdb` `1.10501` → `1.10503`; `russh` `0.60` → `0.61`; plus
  `uuid` and `serde_json` patch bumps.
- **CI workflow actions**: `actions/download-artifact` 4 → 8,
  `softprops/action-gh-release` 2 → 3, `docker/setup-buildx-action` 3 → 4,
  `docker/login-action` 4.1 → 4.2 (release workflow), and
  `actions/deploy-pages` 4 → 5 (pages workflow).

### Notes

- No source or API changes — this release lands accumulated dependency and CI
  maintenance and verifies the bumped release-workflow actions produce a working
  release.

## [1.2.1] — Release test fix

### Fixed

- **tumult-mcp**: update the `agentic_smoke` tool test that pinned the old
  `fake-http-malformed-json` scenario label. The 1.2.0 change routed every
  scenario pack through the real fault-execution engine, so the smoke report's
  scenario is now the bundled pack name (`malformed-json-recovery`). The 1.2.0
  release build's "Run tests before release" step failed on the native
  `x86_64-unknown-linux-gnu` and macOS targets (the musl targets skip test
  execution when cross-compiling, which is why they passed).

## [1.2.0] — Live agentic fault injection + real fault-execution engine

### Added

- **`tumult agentic proxy`**: a fault-injecting HTTP reverse proxy that puts a
  scenario pack's faults into the live model traffic of any base-URL-configurable
  agent. Point Claude Code (`ANTHROPIC_BASE_URL`), the Codex CLI / OpenCode
  (`OPENAI_BASE_URL`), or GitHub Copilot (`HTTPS_PROXY`) at the proxy and watch
  how the real agent copes with injected latency, rate limits, provider errors,
  timeouts, malformed/truncated output, tool failures, and retrieval poisoning.
  Each request is logged and optionally appended to a metadata-only JSONL journal
  with live contract verdicts. New module `tumult-agentic::proxy`.
- **Shared fault-execution engine** (`tumult-agentic::engine`): a single
  `execute` entry point that gates every fault through the seeded `FaultEngine`,
  applies it via `apply_fault`, and evaluates every contract against the
  resulting response.
- **Live-client guide**: `docs/guides/agentic-live-clients.md` documents wiring
  each agent to the proxy and how faults map onto HTTP behaviour.

### Changed

- **Scenario packs now run through the real engine.** `tumult agentic run
  --scenario` previously reported hand-scripted, pre-computed "post-fault"
  responses. Every pack now applies its declared faults through `FaultEngine` +
  `apply_fault` against a per-pack baseline and evaluates its real contracts, so
  the reported pass/fail genuinely reflects what the faults do.
- **`tumult agentic replay --fixture` now replays the supplied fixture.** It
  previously validated the caller's fixture and then discarded it, always
  reporting a built-in smoke fixture. It now runs the caller's fixture end to
  end through the replay adapter; the report's session, source, and step count
  echo that fixture.

### Fixed

- **`retrieval_poisoning` now contaminates the evaluated response body**, not
  just the `retrieved_documents` list, so the body-based safety contracts
  (citation, PII, secret leakage) actually observe the poisoning.

## [1.1.2] — Docker image build fix

### Fixed

- **Docker image**: add `tumult-agentic` and `tumult-intelligence` to the
  `COPY` list in `docker/Dockerfile.tumult`. The 1.1.1 release build's
  "Publish tumult image" job failed because these two workspace crates
  (added in 1.1.0) were never copied into the Docker build context, so
  `cargo build` could not load their manifests as workspace members.

## [1.1.1] — Release build fix

### Fixed

- **Cross-compilation**: pin the workspace `reqwest` dependency to
  `default-features = false` with `rustls-tls` instead of the default
  `native-tls`/OpenSSL backend. The 1.1.0 release build failed for the
  `*-unknown-linux-musl` targets because `tumult-agentic`'s direct `reqwest`
  dependency activated `default-tls`, pulling in `openssl-sys`, which has no
  OpenSSL development headers in the musl cross-compilation containers.
  Switching to `rustls-tls` matches the TLS stack already used everywhere
  else in the workspace and removes ~155 lines of `native-tls`/OpenSSL
  transitive dependencies from `Cargo.lock`.

## [1.1.0] — Agentic Fault Injection

### Added

- **tumult-agentic**: new crate that treats AI agents as systems under test —
  fault injection, behavioral contracts, replay fixtures, scoring, and
  OpenTelemetry correlation for agent workflows that call models, tools, MCP
  servers, or retrieval systems.
- **Agent target model**: run against HTTP agents and MCP tools/servers, with
  framework adapters (LangChain, AutoGen, CrewAI, Google ADK) planned.
- **Fault types**: model latency/timeouts/rate limits, malformed or truncated
  output, hallucinated tool calls, tool latency/failure, retrieval poisoning,
  context truncation, token budget exhaustion, and retry-loop pressure.
- **Behavioral contracts**: validity, safety, fallback behavior, citation
  presence, schema conformance, retry budget, task success, latency, and cost
  controls.
- **Deterministic replay**: turn captured agent traces or production sessions
  into regression experiments.
- **Scenario packs**: concurrency storm, hallucination under timeout, cost
  explosion detector, malformed JSON recovery, tool timeout fallback, and
  retrieval poisoning — all runnable locally without an external LLM via
  `tumult agentic list-packs` and `tumult agentic smoke`.
- **CLI surfaces**: `tumult agentic smoke`, `tumult agentic run --scenario`,
  and `tumult agentic replay --fixture`, all writing metadata-only journals
  with trace correlation and contract evidence.
- **MCP integration**: new tool support for discovering and running agentic
  fault scenarios through the MCP server.
- **Analytics**: new ingestion tables for agent runs, contract checks, fault
  injections, replay outcomes, and resilience scores in `tumult-analytics`.
- **Telemetry**: agentic spans keep Tumult's `resilience.*` experiment
  attributes and add GenAI-aligned `gen_ai.*` attributes for operation, model,
  tool, provider, and evaluation correlation. Raw prompts, completions, tool
  payloads, and retrieved documents default to metadata-only capture.
- **Docs**: Agentic Quickstart, Agentic Observability, and Agentic Scenarios
  guides, plus runnable examples under `examples/agentic/`.

## [1.0.3] — Release workflow fix

### Fixed

- **Release workflow**: filter `download-artifact` step to `tumult-*` pattern so internal `.dockerbuild` layer-cache artifacts are no longer mixed in with release binaries. Fixes the "Create Release" failure that blocked every release since v0.5.0.

### Notes

- No runtime or API changes. This release exists to land the CI fix and produce a working GitHub Release with binary assets.
- All changes accumulated under tags v1.0.0, v1.0.1, and v1.0.2 (none of which published successfully) are included. v1.0.2 was blocked by GitHub's immutable-releases feature locking the tag after a partial release creation.

## [1.0.2] — Unreleased

Tag exists but no GitHub Release published — the immutable-releases feature locked the tag after a partial release creation. See 1.0.3 for the fix.

## [1.0.1] — Unreleased

Tag exists but no GitHub Release was ever published — the release workflow failed at the "Download all artifacts" step. See 1.0.2 for the fix.

## [1.0.0] — Unreleased

Tag exists but no GitHub Release was ever published — same root cause. See 1.0.2 for the fix.

## [0.3.0] — Phase 2: Platform Plugins + Analytics

### Added

- **tumult-analytics**: Embedded analytics crate
  - TOON Journal → Arrow RecordBatch conversion
  - DuckDB embedded SQL engine with zero-copy Arrow ingestion
  - `tumult analyze` command with default summary + custom SQL queries
  - `tumult export` command for Parquet and CSV export
  - ADR-008: Arrow + DuckDB as embedded analytics engine

- **tumult-kubernetes**: Native Kubernetes chaos plugin (kube-rs 3.1)
  - Actions: pod delete, deployment scale, node cordon/uncordon/drain, network policy
  - Probes: pod ready, pods by label, deployment status, node status, service endpoints
  - Label selector targeting (LitmusChaos/Chaos Mesh patterns)
  - ADR-007: Native vs script plugin for Kubernetes

- **tumult-db-postgres**: PostgreSQL chaos plugin
  - Actions: kill connections, lock tables, inject latency, exhaust connection pool
  - Probes: connection count, replication lag, pool utilization

- **tumult-db-mysql**: MySQL chaos plugin
  - Actions: kill connections, lock tables

- **tumult-db-redis**: Redis chaos plugin
  - Actions: FLUSHALL, CLIENT PAUSE, DEBUG SLEEP
  - Probes: redis ping, redis info (connection/memory stats)

- **tumult-kafka**: Kafka broker chaos plugin
  - Actions: kill broker, partition broker, add broker latency
  - Probes: consumer lag, under-replicated partitions, broker count

- **tumult-network**: Network chaos plugin
  - Actions: tc netem latency/loss/corruption, DNS block, host partition
  - Probes: ping latency, DNS resolve

- **tumult-loadtest**: Load testing integration
  - k6 driver with OTLP trace correlation
  - JMeter driver with JTL metrics parsing
  - Example k6 scripts for HTTP and gRPC

### Security

- Input validation library (plugins/lib/validate.sh)
- SQL injection prevention: identifier validation, dollar-quoting
- Credential protection: MYSQL_PWD and REDISCLI_AUTH env vars
- Container runtime allowlist validation

## [0.2.0] — Phase 1: Essential Plugins

### Added

- **tumult-ssh**: SSH remote execution crate
  - Connection manager with russh 0.58 (pure Rust, no C dependencies)
  - Key-based (Ed25519, RSA, ECDSA) and SSH agent authentication
  - Remote command execution with stdout/stderr capture
  - File upload via SSH channel with timeout enforcement
  - Passphrase redaction in Debug output
  - ADR-006: SSH as universal remote transport

- **tumult-stress**: Script plugin for stress-ng
  - Actions: cpu-stress, memory-stress, io-stress, combined-stress
  - Probes: cpu-utilization, memory-utilization, io-utilization
  - Works on both Linux (/proc) and macOS (sysctl/vm_stat)

- **tumult-containers**: Script plugin for Docker/Podman
  - Actions: kill, stop, pause, unpause, limit-cpu, limit-memory
  - Probes: container-status, container-health
  - Supports Docker and Podman via TUMULT_RUNTIME

- **tumult-process**: Script plugin for process chaos
  - Actions: kill (by PID/name/pattern), suspend (SIGSTOP), resume (SIGCONT)
  - Probes: process-exists, process-resources (JSON output)

- Cross-compile release workflow for 6 targets (Linux + macOS)
- serde defaults on all optional fields — minimal experiment files work
- Plugin script test suite (14 tests validating manifests, probes, error handling)

### Fixed

- Init template uses /proc/cpuinfo + /proc/meminfo probes (works out of the box)
- Process timeout enforcement in CLI executor
- Hypothesis probe with tolerance but no output now fails correctly

### Security

- RSA timing side-channel (RUSTSEC-2023-0071) documented with Ed25519 mitigation

## [0.1.0] — Phase 0: Foundation

### Added

- **tumult-core**: Experiment data model with serde/TOON round-trip support
  - All types: Experiment, Activity, Provider, Tolerance, Hypothesis, Journal
  - Five-phase data model: Estimate, Baseline, During, Post, Analysis
  - Execution targets: Local, SSH, Container, KubeExec
  - Config/secret resolution from environment variables and files

- **tumult-core**: Five-phase experiment runner (`runner::run_experiment`)
  - Phase 0 (Estimate): record predictions before execution
  - Phase 1 (Baseline): statistical baseline acquisition
  - Phase 2 (During): method execution with degradation sampling
  - Phase 3 (Post): recovery measurement
  - Phase 4 (Analysis): estimate vs actual accuracy scoring
  - Hypothesis evaluation (before/after) with tolerance matching
  - Rollback strategies: always, on-deviation, never
  - Controls lifecycle: BeforeExperiment, BeforeMethod, BeforeActivity, etc.

- **tumult-baseline**: Statistical baseline derivation
  - Methods: mean +/- N sigma, percentile, IQR, static
  - Anomaly detection (coefficient of variation, extreme range)
  - Tolerance derivation from baseline samples
  - Recovery point detection and compliance ratio

- **tumult-plugin**: Plugin system
  - `TumultPlugin` trait for native Rust plugins
  - Script plugin manifest parser (TOON format)
  - Script execution with TUMULT_* environment variables
  - Plugin discovery from ./plugins/, ~/.tumult/plugins/, $TUMULT_PLUGIN_PATH

- **tumult-otel**: OpenTelemetry instrumentation
  - TracerProvider, MeterProvider, LoggerProvider setup with OTLP
  - tracing-opentelemetry bridge for #[instrument] spans
  - Standard resilience.* namespace attributes
  - Standard metrics: experiments, actions, probes, deviations

- **tumult-cli**: Command-line interface
  - `tumult run` — execute experiments with journal output
  - `tumult validate` — check experiment syntax and references
  - `tumult discover` — list discovered plugins and actions
  - `tumult init` — scaffold new experiments from templates
  - `--dry-run` mode — show execution plan without running
  - Process provider execution (shell scripts)

- **collector/**: Reference OTel Collector configurations
  - Default (stdout), SigNoz, Grafana (Tempo+Mimir+Loki)
  - docker-compose.yaml for local development with Jaeger

- **Documentation**
  - ADR-001 through ADR-009: architectural decisions
  - Experiment format guide
  - Baseline guide
  - Execution flow guide
  - CLI reference
  - Plugin authoring guide
  - Observability setup guide
  - Resilience metadata standard
  - Data lifecycle specification

---

## Earlier releases (analytics platform)

Release history of what is now Tumult's analytics platform. Version
numbers below (≤0.6.0) follow that component's old numbering, not the
Tumult release line.

## [0.6.0] — 2026-07-29

### Added
- **Parquet lake export + retention** (`kronika_store::lake`, ADR 0005):
  incremental, watermark-driven export of `spans`, `logs`, the three
  metric tables and `manual_experiment_audit` to immutable day-partitioned
  parquet files (`<lake>/<table>/date=<d>/data-<run>.parquet`), plus a
  full-snapshot export of `manual_experiments` (fingerprint-gated: skipped
  when the register has not changed since the previous run). Watermarks live in
  `<lake>/_meta.json` (tmp+rename; advanced only after every table
  succeeds → idempotent re-runs). Scheduled in `kronikad` on
  `KRONIKA_LAKE_INTERVAL` (default `24h`, `0`/`off` disables) into
  `KRONIKA_LAKE_DIR` (default `<db dir>/lake`); on-demand via
  `POST /api/lake/export`; status at `GET /api/lake/status`. Retention
  (`KRONIKA_RETENTION_DAYS`, default 0 = keep forever) deletes hot rows
  only when already exported (`ts_ns <= watermark`) through the
  single-writer channel; `manual_experiment_audit` and
  `manual_experiments` are never deleted (append-only compliance
  evidence). Clean-room implementation — OpenObserve (AGPL-3.0) lent
  ideas, no code.
- **Research docs**: `docs/research-openobserve-gap.md` (capability gap +
  AGPL/Apache boundary + borrow-list) and `docs/research-ui-patterns.md`
  (15-pattern observability UI catalog with per-pattern status/effort).
- **Experiment-run chart overlays** (UI patterns 2): `GET
  /api/experiments/windows?from&to` returns runs overlapping a window; the
  Overview (24h) and Metrics charts shade them as outcome-coloured
  `markArea` bands with a start `markLine`, and clicking a band opens the
  run (`web/src/lib/overlays.ts`, one helper used by both pages).
- **Click-to-filter** (UI pattern 1): attribute values in the Logs detail
  rows and the trace SpanDrawer get hover ⊕/⊖ actions that set exact
  `attr=k=v` / `attr_not=k=v` URL params (new `attr`/`attr_not` predicates
  on `/api/logs`, `/api/logs/volume` and `/api/traces` — the latter as an
  `EXISTS` over any span of the trace), shown as removable chips.
- **Correlation legs** (UI pattern 8): the SpanDrawer links to the
  experiment run overlapping the span's window (when the span doesn't
  carry `experiment_id` itself) and correlated logs link to their trace.

### Changed
- **Relicensed MIT → Apache-2.0** (LICENSE, workspace `Cargo.toml`,
  `web/package.json`, README). Added a third-party attributions section
  to the README (ECharts NOTICE, zrender, tslib, Typst, DuckDB, OFL fonts).
- Read-only store connections now document their snapshot semantics: a
  long-lived `Reader` pins its snapshot at open and does not observe later
  commits — open a fresh reader per unit of work.

## [0.5.0] — 2026-07-29

### Added
- **Org hierarchy rollups** (`kronika_docs::org`, ADR 0004): a
  Backstage-style `org.yaml` (`nodes` / `assignments` / `defaults`)
  declares a single-parent tree of arbitrary depth; experiments map to
  teams via `*`-only globs with per-name criticality (critical ×3,
  high ×2). Node scores are criticality-weighted means **recomputed from
  all leaves in the subtree** (never averages of child means) with
  scored/expected coverage; unmapped experiments surface in a visible
  synthetic `(unassigned)` node under the implicit company root. Loaded
  from `KRONIKA_ORG_FILE` (default `<db dir>/org.yaml`); computed at read
  time — no table migration for Part A.
- **`GET /api/scores/tree?node=&range=`** — one node's score, band, delta,
  10-point sparkline and one level of child rollups (weakest first), with
  node-path and range validation.
- **Scores UI page** — KPI cards + ECharts treemap at the root (area ∝
  leaves × criticality, band hues from Okabe–Ito), click-to-drill with
  breadcrumb and `?node=` URL state; indented tree table (score, band
  glyph, Δ, sparkline, coverage, weakest member, weakest-first) below the
  root.
- **Manual evidence** (schema v2, `kronika_store::manual`, ADR 0004): new
  `manual_experiments`, `manual_experiment_audit` and
  `evidence_attachments` tables. Records (game days, tabletops, failovers,
  pentests, drills) move draft → submitted → verified/rejected: full
  mutability in draft only, mandatory attestation on submit, reviewer ≠
  enterer enforcement on verify/reject (same-user → 400), mandatory note
  on reject, and an append-only SHA-256 hash-chained audit trail.
  Attachments are external URIs only (`url`/`ticket`; no file storage).
  IDs are hand-rolled monotonic ULIDs.
- **`/api/manual/*` endpoints** — create/list/get/update drafts,
  submit/verify/reject, attachments, and `POST /api/manual/import`
  (bulk-creates attested **drafts** in one transaction under an
  `import_batches` row — attestation is never bypassed). Mutations ride
  the daemon's single writer via a new `Batch::Exec` ingest variant; the
  API opens no write connection.
- **Manual evidence in scoring and views**: verified records score exactly
  like automated runs (passed 100 / **partial 75** via a new
  `RunState::Partial` / failed 50; **inconclusive is excluded** — it still
  counts toward coverage's expected); drafts/submitted count toward
  coverage as pending with zero weight. `/api/experiments` UNIONs manual
  rows with an `origin` column and origin filter; scoring keys leaves per
  `(name, origin)`.
- **Manual UI page** — two-section entry form (test record + attestation;
  save draft / save & submit), verification queue (verify/reject with
  note), and a records browser with audit-trail and attachment detail. The
  "acting as" name is a plain string (no auth — documented).
- **Reports**: R1 gains a "By domain" table (top-level org children:
  score, band, coverage, weakest) and an evidence-mix footnote ("N
  automated, M verified manual"); R2's test register carries provenance
  (origin, executed vs entered dates, verifier) with a per-entry
  attestation appendix for verified manual records; the content model
  gains `Block::H3`.
- **Demo**: `demo/org.yaml` (three-level hierarchy over the seeded suite,
  one experiment deliberately unassigned) mounted at `/data/org.yaml`; the
  seed service additionally creates a verified manual gameday (alice →
  bob), a submitted tabletop (pending) and a draft drill.
- Docs: `docs/research/research-org-rollups.md`,
  `docs/research/research-manual-evidence.md`,
  `docs/adr/ADR-009-org-hierarchy-and-manual-evidence.md`.

### Changed
- Schema version 1 → 2 (additive; existing stores migrate on open).
- `ApiState` now carries the org tree and (in the daemon) the ingest
  handle; `build_executive` takes an `&OrgTree`.
- kronika-api depends on kronika-ingest for `Batch::Exec`.

### Known limitations
- An experiment name present in both origins yields one scored leaf per
  origin (documented double-count edge). Pending manual records are read
  as-of-now for sparkline history (score history is exact; coverage
  history approximate). Per-child Δ/sparklines are not computed in the
  tree table (cost). No auth — "acting as" names are unverified strings.

---

## [0.4.0] — 2026-07-28

### Added
- **Compliance-grade report pipeline** (`kronika-docs` crate, ADR 0003): a
  renderer-agnostic content model (`ReportDoc`/`Block`/`ChartSpec`) with
  two outputs — embedded-Typst PDFs (typst 0.15 compiled in-process, no
  external runtime) and print-styled HTML previews. Charts are shared
  vector SVGs (Okabe–Ito palette, direct labels); fonts are vendored OFL
  Inter + Source Serif 4 (`assets/fonts/`, embedded in the binary) so
  docker builds stay offline-reproducible.
- **Three report templates**: R1 executive digest (deterministic BLUF,
  portfolio KPIs, target scores, issues discovered/fixed + MTTR, open
  weaknesses, outlook), R3 game-day report (run summary, blast radius &
  rollback, span timeline, verdict, findings, config appendix), and an R2
  evidence-pack skeleton for DORA/NIS2/ISO 27001/SOC 2 with a traceability
  matrix, test register, findings log, sign-off and the mandatory
  "verify clause references against the licensed framework text" footnote.
  Document IDs `KRK-<code>-<yyyymmdd>-<hash6>`; artifacts persisted as
  `{id}.pdf`/`.html`/`.json` with the PDF's SHA-256 in the meta.
- **Resilience scoring** (`kronika_docs::scoring`): Gremlin-style scores
  with 30-day freshness decay (passed 100 / stale 75 / failed 50 / never
  run 0; bands >70 good, 50–70 fair, <50 poor), target and portfolio
  rollups with a period-over-period delta, served at `GET /api/scores`.
- **`/api/reports/v2/*`**: `POST …/generate {type,period?,experiment_id?,
  framework?}`, `GET …/v2` (metas newest first), `GET …/v2/{id}/pdf|html`
  (strict doc-id validation). Integration tests cover the scorecard, all
  three template round-trips and the validation paths.
- **Reports UI v2**: template picker with conditional
  framework/experiment/period controls, artifact list with type badge and
  short SHA, iframe print preview, PDF download; quick metric digests kept
  below. Tabular numerals adopted in `theme.css` (`table.data`, `.mono`).
- Docs: `docs/research-compliance.md`, `docs/research-ux.md`,
  `docs/adr/0003-typst-report-pipeline.md`.
- Report visual polish: composed covers (wordmark + accent rule,
  classification chip, prominent period, document control anchored to the
  page bottom), R1 score-trend line and coverage donut, per-experiment bar
  charts with value labels, balanced KPI grids, glyph+label status cells
  (`Cell::glyph`, never hue alone), readable R3 timeline statuses (OTel
  codes mapped), and fraction-width table columns with justification and
  hyphenation disabled in cells.

### Roadmap (deferred from this cycle)
- ⌘K command palette (wants a global id-search backend first).
- Brush/range-select on Overview charts (needs a coordinated selection
  model; likely with Phase-2 Mosaic crossfiltering).
- BubbleUp-style "explain this spike" drill-downs.
- Notebook-style ad-hoc reports on top of the v2 content model.
- R4/R5 templates (service deep-dive, regulator run-log) pending auditor
  feedback; triage inbox for open weaknesses.

---

### Added
- Logs explorer: `GET /api/logs` (range/severity/service/q/limit; severity a
  case-insensitive exact match, `q` an escaped contains-match; newest first,
  `experiment_id` lifted from log attributes for linking) and
  `GET /api/logs/volume` (bucketed counts per severity). `/logs` UI page with
  a stacked-bar volume chart, URL-synced filters and expandable rows exposing
  attributes plus experiment/trace links.
- Traces explorer: `GET /api/traces` (spans grouped into traces — root
  name/service, span/error counts, experiment outcome where applicable;
  service/min-duration/outcome filters), `GET /api/traces/durations`
  (root-span duration points plus p50/p95/p99 via `quantile_cont`) and
  `GET /api/traces/{id}` (every span and log of one trace). `/traces` UI page
  with a log-scale duration scatter (percentile mark lines, click-through)
  and a slowest-first table; `/traces/[id]` reuses the waterfall and span
  drawer.
- Raw metrics explorer: `GET /api/metrics/catalog` (names across
  sums/gauges/histograms with the attribute keys seen on their points) and
  `GET /api/metrics/query` (bucketed series; sums `SUM`, gauges `AVG`,
  histograms aggregate avg plus an interpolated p95 computed in Rust;
  optional split by a strict-charset attribute key; unknown names 404).
  `/metrics` UI page with typed picker, group-by dropdown, line/area/bar
  toggle, interval and range controls.
- Topology: `GET /api/topology` (service/target nodes with runs/errors/avg
  aggregates from `service_name` and tumult's `resilience.target.name`
  attribute; edges from parent→child span joins and service→target calls;
  capped at 100 nodes). `/topology` UI page with a force-directed graph —
  node size by span count, services colored by error rate, click-through
  from a service to its traces.
- Grounded LLM narratives (`kronika_report::narrative`): a facts package
  built from the report's own KPI/table numbers goes to the LLM; only
  sentences whose numerals are grounded in those facts survive (percent
  matches `x` and `x/100` forms; 1% tolerance for rounding). Unreachable
  LLM, 30s timeout or a fully ungrounded reply leaves the digest unchanged.
  Wired into the daemon's report scheduler and `POST /api/reports/generate`.
  ADR 0002 updated: Phase 2 landed.

### Changed
- `EChart.svelte` accepts an optional click handler; ECharts registers the
  scatter and graph charts.

---

## [0.2.0] — 2026-07-28

### Added
- `kronika-api`: read-only JSON query API backing the UI, mounted on the
  daemon's HTTP server — `GET /api/overview` (KPI cards with value, delta vs
  the previous equal window and sparklines; experiments per day; target
  leaderboard; fault breakdown), `GET /api/timeseries` (any semantic metric
  as a bucketed series), `GET /api/experiments` + `/api/experiments/{id}`
  (outcome joined from tumult's `experiment.completed` log attributes; spans,
  correlated logs and metric points for the waterfall), `GET /api/dimensions`,
  `GET /api/metrics`, `POST /api/ask`, `GET /api/reports[/{name}]`. Every
  query runs on a fresh read-only connection inside `spawn_blocking`.
- `kronika_metrics::to_sql_bucketed` — compile a metric definition into a
  time-bucketed series query (integer-division buckets on `time_col`).
- Web UI (`web/`): SvelteKit 2 + Svelte 5 static SPA (adapter-static,
  `200.html` fallback) — Overview (KPI row, calendar heatmap, fault donut,
  target leaderboard), Experiments (URL-synced filters), experiment detail
  with the custom span waterfall (ruler, indented tree, status-coloured
  bars, click-through drawer with attributes, events and correlated logs),
  Ask (golden answers without an LLM; graceful setup hint when
  `{configured:false}`), Reports. ECharts tree-shaken to bar/heatmap/pie;
  hand-rolled near-black theme. `package-lock.json` committed.
- `kronikad` rust-embeds `web/build/` and serves the SPA on the HTTP port
  (fingerprinted assets cached immutably; non-API paths fall back to the app
  shell). Dockerfile gains a `node:22-bookworm-slim` web stage so the full
  UI ships in the one-command demo at `http://localhost:14318/`.
- Automatic reporting: `KRONIKA_REPORT_INTERVAL` (e.g. `1h`, off by default)
  makes the daemon render a metric digest per interval into
  `<db dir>/reports/report_<epoch>.html`; `/api/reports` lists them and the
  demo compose enables it with `1h`.
- `/api/ask` NL→SQL: curated golden question bank, LLM fallback through
  `kronika-ai`'s OpenAI-compatible client with the sql_guard pipeline
  (allow-listed tables, single SELECT, injected `LIMIT 500`), 30s LLM and
  15s query wall-clock bounds.

---

## [0.1.0] — 2026-07-28

### Added
- Scaffold: Rust workspace with six library crates and the `kronikad` daemon.
- `kronika-store`: embedded DuckDB store (bundled), schema v1 with wide
  ClickHouse-exporter-aligned tables (`spans`, `logs`, `metric_sums`,
  `metric_gauges`, `metric_histograms`, `import_batches`), `MAP` attribute
  columns, `experiment_runs` rollup view, single-writer + read-only reader
  model with `StoreLocked` mapping (tumult-analytics pattern, 0o700 dir).
- `kronika-otel`: pure OTLP → row translation promoting `resilience.*` and
  `service.name` attributes into materialized columns.
- `kronika-ingest`: OTLP/gRPC (`:4317`) and OTLP/HTTP (`:4318`, `/v1/*`,
  `/healthz`) servers funneling through a bounded single-writer channel;
  manual importers for CSV and tumult journal JSON.
- `kronika-metrics`: YAML semantic metric layer compiled to strictly
  identifier-validated SQL; starter definitions in `metrics/`
  (hypothesis_pass_rate, experiment_count, deviation_rate, mttr, coverage,
  action_duration_p95 placeholder).
- `kronika-report`: report model, self-contained HTML renderer, tokio
  interval scheduler.
- `kronika-ai`: Phase 1 groundwork — `Llm` trait, OpenAI-compatible client
  (Ollama default), SQL guardrail pipeline.
- `kronikad`: `serve` (default), `import <file>`, `report --metric <name>`.
- `web/` SvelteKit skeleton (hand-written; not installed), `docs/` (research,
  architecture, ADRs 0001–0002), optional otel-collector dev compose.
- Docker demo: the pinned tumult v2.18.0 release binary (fetched from GitHub
  releases, checksum-verified against `SHA256SUMS.txt` at image build) runs
  the real experiment suite in `demo/experiments/` — eight `.toon`
  experiments (six pass, one deviates, one fails; both rolled back) emitting
  genuine OTLP/gRPC into kronikad; HTML reports land in `demo-out/`. The
  synthetic `kronika-demo` generator remains as optional backfill behind
  `--profile synthetic`.
- `kronikad`: live `GET /report?metric=<name>` endpoint (DuckDB is
  single-process read-write, so reports against a running daemon must be
  served by the daemon); `report --out <file>`.
- `kronika-metrics`: rate terms accept AND-lists of equality conditions.
- `kronika-report`: dimensioned metrics render real breakdown tables; the
  headline KPI uses an ungrouped query.
- `kronika-otel`/`kronika-store`: promote the keys tumult actually emits
  (`resilience.experiment.title`, `resilience.plugin.name`) alongside the
  metadata-standard names; `metric_histograms` gains promoted-dim columns
  (idempotent `ALTER` for existing stores).

### Changed
- Semantic metrics retargeted at tumult's real wire emission:
  `hypothesis_pass_rate` and `deviation_rate` compute over the
  `tumult.experiments.total` / `tumult.hypothesis.deviations.total`
  counters; new `experiment_duration_s`, `experiment_coverage` and
  `action_duration_s`; the span-based `mttr`, `coverage` and
  `action_duration_p95` definitions remain for the synthetic profile.

### Roadmap
- Web UI data plumbing + span-waterfall component.
- Report delivery (email/webhook) and static chart rendering.
- Mosaic crossfiltering (Phase 2, pinned + wrapped), Perspective widget.
- AI phases: NL query → narrative digests → anomaly explanation → insights.
- Parquet lake export partitioned by date.

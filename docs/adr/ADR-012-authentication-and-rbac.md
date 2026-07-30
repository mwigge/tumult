---
title: "ADR-012: Authentication and RBAC"
parent: Architecture Decisions
nav_order: 12
---

# ADR-012: Authentication and RBAC (server-side sessions, argon2id, scoped tokens)

- Status: accepted
- Date: 2026-07-30

## Context

Krönika's API, UI, and ingest endpoints were unauthenticated: anyone who
could reach the HTTP port could read telemetry, mutate manual evidence,
generate reports, and — after ADR-011 — enqueue and stop chaos experiments.
The MCP server already had a token + role model (`viewer`/`operator`) and a
fail-closed non-loopback bind guard; the daemon had neither. As soon as the
daemon runs experiments, "who asked for this" stops being optional:

- **Blast radius is authenticated territory.** Enqueueing a fault-injection
  run is a privileged act; the API must know who did it and record it.
- **Evidence integrity.** Manual-evidence records (`entered_by`,
  `reviewer`) were free-text strings — unusable as attestation.
- **Compliance.** DORA/ISO 27001 evidence trails presuppose identified
  actors and access control.

Constraints inherited from the platform: embedded single-node DuckDB (no
external identity store), pure-Rust dependency rule, single-writer store
channel, and the crash-robustness lesson of schema v5 (no secondary indexes
on daemon-written tables).

## Decision

### One shared crate: `tumult-auth`

All authentication primitives live in a new `tumult-auth` crate, consumed
by the daemon API, the MCP server, and the ingest path: the `Role` enum,
argon2id password hashing, opaque id/token generation, sha256/constant-time
helpers, and the `host_is_loopback` bind-policy check. The MCP server's
local `Role` enum was replaced by a re-export of the shared one.

### Passwords: argon2id at OWASP parameters

Passwords are hashed with argon2id (m = 19 MiB, t = 2, p = 1) and stored as
PHC strings. Login verifies in `spawn_blocking` (the hash is deliberately
CPU-expensive) and always performs the same work — an unknown username
verifies against a cached dummy hash — so the endpoint reveals neither
which usernames exist nor (through timing) whether the user was found.

### Sessions: opaque, server-side, hashed at rest

Browser sessions are 256-bit opaque ids. The store keeps only the sha256 of
the id, so a database read (or leak) does not yield usable session cookies.
Cookies are `HttpOnly; SameSite=Strict; Path=/`, 12-hour absolute expiry,
`Secure` whenever the daemon binds a non-loopback address. No JWTs: there
is nothing to revoke-list, logout is a row delete, and the platform has no
need for stateless cross-service token verification.

### Machine tokens: `kro_`-prefixed, hashed at rest

Automation authenticates with `kro_`-prefixed API tokens
(`Authorization: Bearer`), also stored only as sha256. Tokens belong to a
user, carry that user's role and environment scopes, are individually
revocable, and stamp `last_used_at_ns` best-effort. The prefix makes them
recognisable to secret scanners and in logs.

### RBAC: four roles, a route table, fail closed

Roles are `viewer < operator < approver < admin` (derived `Ord`; a higher
role satisfies every lower requirement). Role *parsing* is fail-closed: an
unrecognised role at config or API level is rejected, never defaulted
upwards. Authorization is a hand-rolled axum middleware plus a single
`ROUTE_TABLE` (method, path template, minimum role) — no external policy
engine. The table's default for an unmatched route is `admin`, so adding a
route without classifying it fails closed, and a test sweeps every mutating
table entry for a 401 without credentials plus a route×role matrix.

- **viewer** — all reads, ask, dry-run/validate, own password.
- **operator** — enqueue/stop runs, journal import, manual-evidence
  authoring and submission, report generation, lake export.
- **approver** — verify/reject manual evidence (segregation of duties: the
  store already enforces reviewer ≠ enterer).
- **admin** — user and token administration.

The MCP two-tier gate is unchanged; `approver` and `admin` tokens pass it
by the derived ordering.

### Environment scopes

Each user may carry a set of allowed environments (`user_env_scopes`;
empty = all environments). Scoped users see only their environments in
experiment and run lists/details; runs not yet linked to an experiment
(queued) remain visible to everyone — they carry no environment until
execution starts.

### Identity in the audit trail

Run audit events and manual-evidence mutations record the authenticated
username (`run_audit.actor`, schema v6). When auth is not configured the
manual-evidence API keeps accepting free-text actors exactly as before, so
existing integrations are unaffected.

### Bootstrap and the bind guard

Auth is **enabled iff at least one real user exists** — checked per
request. The `legacy` backfill identity seeded by the v6 migration for
pre-auth free-text actors is disabled, unverifiable (`password_hash = '!'`),
and does not count: an upgraded pre-auth store keeps working until an
admin is created, instead of locking itself out on first open.

- `tumultd create-admin` creates the first admin with a one-time password
  (`must_change` forces replacement at first login) while the daemon is
  stopped.
- Ported from the MCP server: a non-loopback bind with no users and no
  bootstrap env var refuses to start. `KRONIKA_BOOTSTRAP_ADMIN_PASSWORD`
  (and `KRONIKA_BOOTSTRAP_TOKEN`) exist for demos and dev only, and log a
  loud warning when used.
- `KRONIKA_INGEST_TOKEN` guards the OTLP `/v1/*` HTTP routes and gRPC
  export methods (constant-time compare; `/healthz` stays open); clients
  send it via the standard `OTEL_EXPORTER_OTLP_HEADERS` env var, and the
  tumult CLI sends `TUMULT_DAEMON_TOKEN` on journal import.

## Consequences

- Every mutating API route is authenticated and role-gated once any user
  exists; before that, the API behaves exactly as it did pre-auth (the
  middleware injects a synthetic admin), keeping upgrades and loopback dev
  friction-free.
- No identity provider, no sessions table to replicate, no token refresh
  dance — the model fits a single-node embedded store. Federated identity
  (OIDC) remains a possible future layer in front of the same middleware.
- The 12-hour session has no sliding renewal; re-login is required after
  expiry. Login is not rate-limited at the application layer (the argon2
  work factor is the primary brake) — a reverse proxy limit is recommended
  for exposed deployments.
- Auth tables are index-free under the same crash-robustness rule as the
  v5 run tables; uniqueness is enforced in code behind the single writer.

## Follow-ons

- [ADR-013](ADR-013-approval-workflows-and-hash-pinning.md) builds the
  approval workflow on this role model: the `approver` role exists for its
  quorum decisions, and its break-glass override is Admin-only under this
  route table.

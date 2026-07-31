---
title: "ADR-006: Analytics Platform Stack"
parent: Architecture Decisions
nav_order: 6
---

# ADR-006: Analytics Platform Stack

- Status: accepted
- Date: 2026-07-28

## Context

The analytics layer of Tumult sits next to the tumult CLI and smedja. It
must ingest OTLP from both exporters (gRPC and HTTP/protobuf), store
telemetry locally with strong ad-hoc query capability, and serve a
presentation-first web UI. The question is where the query engine lives
and what serves the API.

Options considered:

1. **Rust server-side: axum + tonic + embedded DuckDB (duckdb-rs, bundled)**
2. **DuckDB-WASM in the browser**, with files shipped to the client
3. **Perspective-centered app** (@finos/perspective) with a thin data feed

## Decision

Option 1 — Rust server-side with axum (OTLP/HTTP + API), tonic (OTLP/gRPC)
and embedded DuckDB, mirroring `tumult-lake`'s single-writer pattern.

## Rationale

- **Ingest is server-side anyway.** Both exporters push OTLP to a collector
  endpoint; a browser-first architecture (DuckDB-WASM) has no story for
  receiving OTLP/gRPC streams and would still need a server ingest process
  that owns the file — at which point the server might as well own queries
  too.
- **DuckDB-WASM trade-offs.** WASM shines for client-heavy, offline-first BI
  over static files. Our data is continuously appended by a daemon, and
  the single-writer file model means a browser cannot open the live
  store. Shipping snapshots to the browser duplicates the lake-export
  roadmap item for no interactive win.
- **Perspective-centered trade-offs.** Perspective is an excellent streaming
  pivot/exploration widget, but centering the product on it yields the
  generic-BI aesthetic we deliberately position against, and it does not
  provide the presentation-first report/digest layer or the signature
  span-waterfall. Perspective remains an *optional* ad-hoc exploration
  widget (see `web/README.md`).
- **Ecosystem alignment.** axum 0.8, tonic 0.14, prost 0.14 and
  opentelemetry-proto 0.32 match the versions already resolved in the
  workspace lockfile; duckdb 1.10503 (bundled) matches `tumult-lake`. One
  toolchain, one vocabulary (`resilience.*`), shared patterns
  (single-writer, `StoreLocked`, 0o700 store dir).

## Consequences

- The web UI talks to a JSON API on the daemon (Phase 2; v1 UI is a static
  skeleton) instead of querying a local engine — slightly more server code,
  full control over the query surface.
- Mosaic crossfiltering (Phase 2) will run over the server-side store via
  the selection abstraction, not over DuckDB-WASM in the browser.
- Binary workspace commits `Cargo.lock` (tumult convention).

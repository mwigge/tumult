# Research: the OpenObserve gap (v0.6.0)

Status: analysis — the parquet lake export + retention job (v0.6.0) is the
first item built from the borrow-list below; see ADR 0005.

## Why look at OpenObserve at all

OpenObserve is the closest full product to kronika's storage layer: an
embedded-friendly observability store that keeps a hot index and offloads
columnar files to object storage, with SQL on top. Comparing against it
clarifies what kronika deliberately is (a compliance-grade resilience
chronicle) and what it cheaply can borrow (durability and lifecycle ideas).

**License boundary, stated once and applied everywhere:** OpenObserve is
**AGPL-3.0**; kronika is Apache-2.0. Ideas, architectures and feature
concepts are not copyrightable and may be borrowed freely. Code may not:
no copying, no porting, no line-by-line "translation" of any OpenObserve
source, no consulting its implementation while writing ours. Everything
kronika ships in this area is **clean-room**, implemented against DuckDB's
own documented behaviour (`COPY … TO … (FORMAT PARQUET)`) and the ideas
below. Where a design decision resembles OpenObserve's, that is convergent
thinking from the same constraints, not derivation.

## Capability comparison (summary)

What kronika already has that OpenObserve does not:

- **Typst PDF compliance reports** (R1 executive digest, R2 evidence pack,
  R3 experiment card) with document IDs and SHA-256 integrity.
- **Manual evidence**: attested hand-entered test records with a
  draft → submitted → verified/rejected lifecycle, reviewer ≠ enterer, and
  an append-only hash-chained audit trail.
- **Org rollups**: criticality-weighted scores recomputed from leaves over
  a declarative YAML hierarchy, with coverage denominators.
- **A YAML semantic layer**: named metrics and report templates as data,
  not dashboard JSON.

What OpenObserve has that kronika lacks (the honest gap):

| Capability | OpenObserve | kronika | Priority read |
| --- | --- | --- | --- |
| Retention / data lifecycle | per-stream retention + compaction | none — hot store grows forever | **high** (v0.6.0) |
| Columnar lake export | parquet on object storage | none | **high** (v0.6.0) |
| Full-text search | inverted index over logs | `ILIKE` substring scan | medium |
| Alerting | scheduled + real-time alerts | none (reports only) | medium |
| Pipelines / enrichment | VRL transform pipelines | none | low |
| RBAC / SSO | built-in, org-scoped | none (documented "no auth") | site-dependent |
| HA / clustering | raft + horizontal scale | single-node embedded | out of scope |
| Dashboard builder | drag-drop panels | fixed pages + YAML metrics | deliberately skipped |
| PromQL | supported | SQL only | low |

## The borrow-list

Three ideas are worth taking now; each is reframed for a compliance tool
rather than copied as a feature.

1. **Parquet export + retention/compaction.** A hot embedded store with no
   lifecycle story eventually fills its disk — that is the single largest
   operational gap in kronika. The idea to borrow is the *shape*: hot rows
   for fast recent queries, immutable columnar files for the long tail,
   and retention that reclaims the hot store only after the cold copy
   exists. Our implementation is DuckDB-native: `COPY (SELECT …) TO
   '<lake>/<table>/date=<d>/data-<ts>.parquet'`, one file per day-partition
   directory, incremental against a persisted watermark, with retention
   deletes gated on the watermark so nothing unexported is ever dropped.
2. **Immutability as a feature, not a limitation.** OpenObserve's lake
   files are write-once because that is what object storage gives you;
   kronika reframes the same property as *compliance evidence*: parquet
   files that are never rewritten, next to the existing hash-chained
   manual-evidence audit, give a tamper-evident durability story
   ("WORM-shaped") that a mutable hot store cannot. The manual audit table
   is exported but **never** deleted by retention — append-only in hot and
   cold tiers alike.
3. **WAL / durability documentation.** OpenObserve documents its write-ahead
   behaviour as an operator-facing guarantee. Kronika's equivalent
   (DuckDB's ACID single-writer hot store + the export watermark's
   crash-safety properties) deserves the same treatment: written down in
   `docs/architecture.md` so operators know exactly what happens to rows
   between ingest, checkpoint, export and retention.

## Deliberately not borrowed

The dashboard builder (kronika's opinionated fixed pages are the product),
the VRL pipeline language (ingest-time enrichment belongs in tumult/smedja
exporters), clustering (contradicts the embedded single-writer model), and
PromQL (kronika's semantic layer is SQL-over-YAML by design). Alerting and
FTS are real gaps but sit behind the lake work; they are noted here so the
priority order is explicit rather than accidental.

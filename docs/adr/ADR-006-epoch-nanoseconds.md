# ADR-006: Epoch Nanoseconds for Timestamps

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

The platform needs a canonical time format for timestamps in journals, OTel spans, and analytics queries. Options considered: ISO 8601 strings (human-readable, 28 bytes, requires parsing), Unix epoch seconds (compact but insufficient precision for sub-second probes), epoch milliseconds (common in JavaScript ecosystems), and epoch nanoseconds (OTel native, ClickHouse/DuckDB native). Durations also need a canonical format for both machine processing (engine internals) and human authoring (experiment definitions).

## Decision

Use epoch nanoseconds (int64) as the canonical timestamp format for all internal representations: journals, OTel spans, analytics storage, and API responses. Use float64 seconds as the canonical duration format for machine processing. Accept human-friendly duration syntax (`120s`, `5m`, `2h30m`) in experiment definitions -- the engine converts these to float64 seconds at parse time.

## Consequences

### Positive
- Native format for OpenTelemetry spans -- zero conversion needed between Tumult journals and OTel exporters
- Native format for ClickHouse and DuckDB timestamp columns -- zero conversion for analytics queries
- Integer arithmetic for time correlation (span start + duration = span end) with nanosecond precision
- 8 bytes per timestamp vs 28 bytes for ISO 8601 strings -- significant savings in high-frequency probe journals
- Human-friendly duration syntax in experiment definitions preserves authoring ergonomics

### Negative
- Raw journal files are not human-readable for timestamps; requires a display layer or CLI formatting command
- int64 epoch nanoseconds overflow in the year 2554 -- not a practical concern but worth documenting

### Risks
- Contributors may accidentally use milliseconds or seconds when nanoseconds are expected; the type system and documentation must make the unit explicit
- External systems consuming Tumult data may expect ISO 8601 or epoch milliseconds, requiring a conversion layer in export paths

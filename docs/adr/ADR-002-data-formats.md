---
title: "ADR-002: Data Formats"
parent: Architecture Decisions
nav_order: 2
---

# ADR-002: Data Formats: TOON, resilience.* Namespace, and Epoch Nanoseconds

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Three interrelated data format decisions shape how the platform represents, names, and timestamps all structured data.

**Data serialization format.** Chaos experiments, journals, configuration files, and plugin manifests all require a structured data format. Chaos Toolkit uses JSON exclusively. YAML is a common alternative offering better human readability. TOON is a newer token-efficient format with full serde support via the `toon-format` crate (v0.4.4, TOON spec v3.0). A key consideration is that experiment definitions and journals are frequently analyzed by LLMs for incident response, resilience scoring, and automated remediation -- token efficiency directly impacts cost and context window utilization.

**Metadata namespace.** The platform needs a standard attribute namespace for chaos engineering metadata in OTel spans, metrics, and logs. Three options were considered: `tumult.*` (product-specific, limits adoption by other tools), `chaos.*` (domain-specific but carries negative connotations in regulatory and executive contexts), and `resilience.*` (domain-aligned with DORA, NIS2, ISO 22316, and PCI-DSS terminology). The namespace must be suitable for regulatory evidence, cross-tool interoperability, and long-term community adoption.

**Timestamp format.** The platform needs a canonical time format for timestamps in journals, OTel spans, and analytics queries. Options considered: ISO 8601 strings (human-readable, 28 bytes, requires parsing), Unix epoch seconds (compact but insufficient precision for sub-second probes), epoch milliseconds (common in JavaScript ecosystems), and epoch nanoseconds (OTel native, ClickHouse/DuckDB native). Durations also need a canonical format for both machine processing (engine internals) and human authoring (experiment definitions).

## Decision

### TOON as Primary Data Format

Use TOON as the primary data format for experiments, journals, configuration, and plugin manifests. The `toon-format` crate provides serde serialization and deserialization. JSON and YAML are accepted as input formats for migration convenience but TOON is the canonical output format.

### resilience.* Metadata Namespace

Use `resilience.*` as the community metadata namespace for all chaos engineering and resilience validation telemetry. Attributes follow the pattern `resilience.<component>.<attribute>` (e.g., `resilience.experiment.name`, `resilience.action.type`, `resilience.probe.latency_ns`).

### Epoch Nanoseconds for Timestamps

Use epoch nanoseconds (int64) as the canonical timestamp format for all internal representations: journals, OTel spans, analytics storage, and API responses. Use float64 seconds as the canonical duration format for machine processing. Accept human-friendly duration syntax (`120s`, `5m`, `2h30m`) in experiment definitions -- the engine converts these to float64 seconds at parse time.

## Consequences

### Positive

**TOON format:**
- Approximately 40-50% token reduction compared to equivalent JSON when processed by LLMs
- Human-readable syntax with less punctuation noise than JSON
- Full serde compatibility via `toon-format` crate -- same derive macros as JSON/YAML
- Tabular array syntax is well-suited for probe result series and metric snapshots
- Smaller file sizes reduce storage and transfer costs for experiment journals

**resilience.* namespace:**
- Direct regulatory alignment: DORA uses the term "resilience" over 200 times; NIS2, ISO 22316, and PCI-DSS all frame requirements in resilience terms
- Vendor-neutral and tool-neutral: any chaos or resilience tool can adopt the namespace without implying Tumult dependency
- Broader scope than "chaos" -- covers steady-state validation, recovery measurement, and compliance evidence, not just fault injection
- Positive framing for executive and audit audiences ("resilience testing" vs "chaos testing")

**Epoch nanoseconds:**
- Native format for OpenTelemetry spans -- zero conversion needed between Tumult journals and OTel exporters
- Native format for ClickHouse and DuckDB timestamp columns -- zero conversion for analytics queries
- Integer arithmetic for time correlation (span start + duration = span end) with nanosecond precision
- 8 bytes per timestamp vs 28 bytes for ISO 8601 strings -- significant savings in high-frequency probe journals
- Human-friendly duration syntax in experiment definitions preserves authoring ergonomics

### Negative

**TOON format:**
- New format with less ecosystem tooling than JSON or YAML (no native browser rendering, limited editor support)
- Team learning curve for reading and authoring TOON files directly
- Third-party integrations expecting JSON will require a conversion layer

**resilience.* namespace:**
- Less specific than `chaos.*` -- could be confused with unrelated resilience tooling (circuit breakers, retry libraries)
- May overlap with future OTel semantic conventions if the community standardizes resilience attributes independently

**Epoch nanoseconds:**
- Raw journal files are not human-readable for timestamps; requires a display layer or CLI formatting command
- int64 epoch nanoseconds overflow in the year 2554 -- not a practical concern but worth documenting

### Risks
- TOON spec and crate are relatively new; breaking changes in future spec versions could require migration effort
- Editor and IDE support may remain limited if the format does not achieve broader adoption
- If OTel defines an official `resilience.*` semantic convention, Tumult's namespace may need to be reconciled or migrated
- Adoption of `resilience.*` outside Tumult depends on community buy-in; the namespace has value only if multiple tools use it
- Contributors may accidentally use milliseconds or seconds when nanoseconds are expected; the type system and documentation must make the unit explicit
- External systems consuming Tumult data may expect ISO 8601 or epoch milliseconds, requiring a conversion layer in export paths

# ADR-002: TOON Data Format

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Chaos experiments, journals, configuration files, and plugin manifests all require a structured data format. Chaos Toolkit uses JSON exclusively. YAML is a common alternative offering better human readability. TOON is a newer token-efficient format with full serde support via the `toon-format` crate (v0.4.4, TOON spec v3.0). A key consideration is that experiment definitions and journals are frequently analyzed by LLMs for incident response, resilience scoring, and automated remediation -- token efficiency directly impacts cost and context window utilization.

## Decision

Use TOON as the primary data format for experiments, journals, configuration, and plugin manifests. The `toon-format` crate provides serde serialization and deserialization. JSON and YAML are accepted as input formats for migration convenience but TOON is the canonical output format.

## Consequences

### Positive
- Approximately 40-50% token reduction compared to equivalent JSON when processed by LLMs
- Human-readable syntax with less punctuation noise than JSON
- Full serde compatibility via `toon-format` crate -- same derive macros as JSON/YAML
- Tabular array syntax is well-suited for probe result series and metric snapshots
- Smaller file sizes reduce storage and transfer costs for experiment journals

### Negative
- New format with less ecosystem tooling than JSON or YAML (no native browser rendering, limited editor support)
- Team learning curve for reading and authoring TOON files directly
- Third-party integrations expecting JSON will require a conversion layer

### Risks
- TOON spec and crate are relatively new; breaking changes in future spec versions could require migration effort
- Editor and IDE support may remain limited if the format does not achieve broader adoption

# ADR-005: Resilience Metadata Namespace

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

The platform needs a standard attribute namespace for chaos engineering metadata in OTel spans, metrics, and logs. Three options were considered: `tumult.*` (product-specific, limits adoption by other tools), `chaos.*` (domain-specific but carries negative connotations in regulatory and executive contexts), and `resilience.*` (domain-aligned with DORA, NIS2, ISO 22316, and PCI-DSS terminology). The namespace must be suitable for regulatory evidence, cross-tool interoperability, and long-term community adoption.

## Decision

Use `resilience.*` as the community metadata namespace for all chaos engineering and resilience validation telemetry. Attributes follow the pattern `resilience.<component>.<attribute>` (e.g., `resilience.experiment.name`, `resilience.action.type`, `resilience.probe.latency_ns`).

## Consequences

### Positive
- Direct regulatory alignment: DORA uses the term "resilience" over 200 times; NIS2, ISO 22316, and PCI-DSS all frame requirements in resilience terms
- Vendor-neutral and tool-neutral: any chaos or resilience tool can adopt the namespace without implying Tumult dependency
- Broader scope than "chaos" -- covers steady-state validation, recovery measurement, and compliance evidence, not just fault injection
- Positive framing for executive and audit audiences ("resilience testing" vs "chaos testing")

### Negative
- Less specific than `chaos.*` -- could be confused with unrelated resilience tooling (circuit breakers, retry libraries)
- May overlap with future OTel semantic conventions if the community standardizes resilience attributes independently

### Risks
- If OTel defines an official `resilience.*` semantic convention, Tumult's namespace may need to be reconciled or migrated
- Adoption outside Tumult depends on community buy-in; the namespace has value only if multiple tools use it

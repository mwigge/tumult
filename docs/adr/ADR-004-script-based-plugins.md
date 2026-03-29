# ADR-004: Script-Based Plugins

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Chaos Toolkit plugins are Python modules installed into the same Python environment. In a Rust platform, dynamic loading options are limited: dylib plugins suffer from ABI fragility across compiler versions, and WASM sandboxing actively fights the chaos engineering use case (which requires host-level access to inject faults). The platform needs to support community contributions from operators who may not have a Rust toolchain, while still allowing high-performance native plugins for SDK-dependent integrations like Kubernetes and cloud providers.

## Decision

Community plugins are directories containing a TOON manifest (`plugin.toon`) and one or more executable scripts. Scripts receive arguments as `TUMULT_*` environment variables and return results via stdout (TOON-formatted). Native Rust plugins for performance-critical or SDK-dependent integrations (Kubernetes, AWS, GCP, Azure) are compiled into the binary via Cargo feature flags.

## Consequences

### Positive
- Anyone who can write a shell script can write a Tumult plugin -- no Rust toolchain required
- TOON manifests make plugins discoverable and self-describing (declared actions, probes, required env vars)
- Native Rust plugins for performance-critical paths (high-frequency probes, K8s API interactions) avoid process spawning overhead
- Feature flags keep the core binary small; users compile in only the native plugins they need
- Plugin directories are trivially distributable (tar, git clone, container mount)

### Negative
- Process spawning overhead for script-based plugins adds latency compared to in-process execution
- No compile-time type safety for script plugin interfaces; errors surface at runtime
- Two plugin models (script and native) increase the surface area for documentation and testing
- Script plugins depend on host-installed interpreters (bash, python, etc.)

### Risks
- Script plugin interface may need versioning as the platform evolves, requiring a manifest version field and migration tooling
- Security boundary between script plugins and the host is minimal -- a malicious plugin has full host access

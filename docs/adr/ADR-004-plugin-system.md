---
title: "ADR-004: Plugin System"
parent: Architecture Decisions
nav_order: 4
---

# ADR-004: Plugin System: Script-Based Community Plugins with Discovery

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Chaos Toolkit plugins are Python modules installed into the same Python environment. In a Rust platform, dynamic loading options are limited: dylib plugins suffer from ABI fragility across compiler versions, and WASM sandboxing actively fights the chaos engineering use case (which requires host-level access to inject faults). The platform needs to support community contributions from operators who may not have a Rust toolchain, while still allowing high-performance native plugins for SDK-dependent integrations like Kubernetes and cloud providers.

Additionally, since Tumult supports two plugin types (native Rust plugins compiled in via feature flags and script-based community plugins as directories with a manifest and executable scripts), script plugins can exist in multiple locations -- local to the experiment, global per user, or in custom paths. When the same plugin name appears in multiple locations, the engine must deterministically choose which one to use.

## Decision

### Script-Based Community Plugins

Community plugins are directories containing a TOON manifest (`plugin.toon`) and one or more executable scripts. Scripts receive arguments as `TUMULT_*` environment variables and return results via stdout (TOON-formatted). Native Rust plugins for performance-critical or SDK-dependent integrations (Kubernetes, AWS, GCP, Azure) are compiled into the binary via Cargo feature flags.

### Plugin Discovery Order

Plugin discovery follows this order, with first-found-wins deduplication by name:

1. `./plugins/` -- local to the experiment directory
2. `~/.tumult/plugins/` -- user-global plugins
3. `TUMULT_PLUGIN_PATH` -- colon-separated custom paths (for CI/CD or team-shared plugins)
4. Compiled-in native plugins -- always available regardless of filesystem

When a script plugin and native plugin share the same name, the script plugin wins if discovered first. This allows users to override native behavior with a local script plugin for testing or customization.

## Consequences

### Positive
- Anyone who can write a shell script can write a Tumult plugin -- no Rust toolchain required
- TOON manifests make plugins discoverable and self-describing (declared actions, probes, required env vars)
- Native Rust plugins for performance-critical paths (high-frequency probes, K8s API interactions) avoid process spawning overhead
- Feature flags keep the core binary small; users compile in only the native plugins they need
- Plugin directories are trivially distributable (tar, git clone, container mount)
- Local plugins override global -- expected behavior for project-specific customization
- `TUMULT_PLUGIN_PATH` enables CI/CD pipelines and team-shared plugin repositories
- Native plugins are always available as a fallback
- Discovery is deterministic and predictable

### Negative
- Process spawning overhead for script-based plugins adds latency compared to in-process execution
- No compile-time type safety for script plugin interfaces; errors surface at runtime
- Two plugin models (script and native) increase the surface area for documentation and testing
- Script plugins depend on host-installed interpreters (bash, python, etc.)
- Silent shadowing when a local plugin overrides a global plugin with the same name
- No version conflict resolution -- first found wins regardless of version

### Risks
- Script plugin interface may need versioning as the platform evolves, requiring a manifest version field and migration tooling
- Security boundary between script plugins and the host is minimal -- a malicious plugin has full host access
- Plugin manifest format changes require backward compatibility strategy
- Malicious script plugins in shared paths could execute arbitrary code

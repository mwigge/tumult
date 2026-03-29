# ADR-008: Plugin Discovery Order and Resolution Strategy

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Tumult supports two plugin types: native Rust plugins (compiled in via feature flags) and script-based community plugins (directories with a manifest and executable scripts). Script plugins can exist in multiple locations — local to the experiment, global per user, or in custom paths. When the same plugin name appears in multiple locations, the engine must deterministically choose which one to use.

## Decision

Plugin discovery follows this order, with first-found-wins deduplication by name:

1. `./plugins/` — local to the experiment directory
2. `~/.tumult/plugins/` — user-global plugins
3. `TUMULT_PLUGIN_PATH` — colon-separated custom paths (for CI/CD or team-shared plugins)
4. Compiled-in native plugins — always available regardless of filesystem

When a script plugin and native plugin share the same name, the script plugin wins if discovered first. This allows users to override native behavior with a local script plugin for testing or customization.

## Consequences

### Positive

- Local plugins override global — expected behavior for project-specific customization
- `TUMULT_PLUGIN_PATH` enables CI/CD pipelines and team-shared plugin repositories
- Native plugins are always available as a fallback
- Discovery is deterministic and predictable

### Negative

- Silent shadowing when a local plugin overrides a global plugin with the same name
- No version conflict resolution — first found wins regardless of version

### Risks

- Plugin manifest format changes require backward compatibility strategy
- Malicious script plugins in shared paths could execute arbitrary code

---
title: Execution bindings
parent: Guides
---

# Bind execution inputs to an environment

The daemon no longer trusts a caller's `env` label to lower a fault's approval tier. An unbound execution requires T3 approval, even when labeled `dev` or when its activities are declared as probes. A user with environment scopes cannot launch an unbound definition. This also applies to schedules and GameDay children. Reviewed, bound probe-only plans may receive T0; a caller-supplied probe label is not a sandbox.

An administrator must review the actual provider destinations, configuration and credential sources before authorizing a binding. The binding is an operator-owned JSON file; registering or editing a definition through the API cannot create one.

1. Register the experiment, then open **Runs → New run**. Choose its environment and target, fill its parameters, and dry-run the plan. The preview masks environment-derived configuration and secret values.
2. Review the definition and the server-side connection/configuration sources. Expand **Execution fingerprint for administrator review** to obtain its SHA-256 digest. Only an unrestricted administrator receives this digest in the dry-run API’s `binding_hash` field.
3. Add the approved fingerprint, environment and optional target to the file below. Keep it writable only by the deployment administrator; the daemon requires read access. Replace the file atomically when updating it.

```json
[
  {
    "sha256": "<binding_hash from the administrator’s reviewed dry-run>",
    "env": "staging",
    "target": "database-a"
  }
]
```

Set `TUMULTD_EXECUTION_BINDINGS=/etc/tumult/execution-bindings.json` to select the file. Without that variable the daemon looks next to the database: `/var/lib/tumult/lake.duckdb` uses `/var/lib/tumult/lake.execution-bindings.json`. A missing default file means no bindings. An explicitly configured file that is missing, unreadable or invalid fails closed. `target` may be `null` when no target label is selected; it must match the launch request exactly.

The fingerprint covers the canonical resolved experiment and all injected configuration/secret environment values. Changing template parameters, resolved destinations, or those source values requires review and a new binding. Treat this digest as privileged: it can be used to guess low-entropy values if the rest of the artifact is known. Do not publish binding files, credentials or resolved execution artifacts. Ordinary API clients receive only an opaque `execution_hash` preview token; send it unchanged with the run request. This token expires when the daemon restarts, requiring another dry-run.

A binding permits ordinary tier classification; it does not bypass approvals. Production faults still require T3, and other shape-based risk rules still apply. A request for an environment or target inconsistent with a known binding is rejected. Scheduled executions retain the original author's identity and recheck that the author remains enabled, holds an execution role, and can access the environment.

Bindings do not sandbox arbitrary process or script providers, freeze remote services, or authenticate a destination based on its label. The administrator must control script binaries, deployment credentials and network permissions. Use a narrowly privileged execution environment. Per-customer connection management and isolated execution workers remain separate SaaS work.

## Upgrade behavior

Schema version 15 adds private `run_execution_pins`, stored atomically with the run request. Credential verifiers never enter public audit events, and portable audit exports retain their original verifiable chain. Complete operational backups include the private table; portable archives exclude it. Pending requests from versions without a private execution pin must be submitted again. Existing definitions and schedules remain stored. Executions without a reviewed binding now enter T3 rather than obtaining a lower tier from an arbitrary `dev` label; scoped launches fail with an actionable binding error. Add reviewed bindings before resuming those schedules or campaigns. A user's environment scopes now restrict manual-evidence writes and run controls as well as reads. Global exports, the legacy `/report` live metric endpoint, journal import, webhooks and identity administration require an unscoped identity because those resources do not yet have environment ownership.

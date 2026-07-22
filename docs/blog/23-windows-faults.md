---
title: "Native Windows Faults in Tumult 2.12"
parent: Blog
nav_order: 23
updated: 2026-07-21
---

# Native Windows Faults in Tumult 2.12

*Originally published 2026-07-05; reviewed for Tumult 2.16.1 on 2026-07-21.*

Tumult 2.12 added `tumult-windows`, a native Windows fault provider using the
same experiment and journal formats as the other providers. It currently
implements three focused operations:

```text
process_kill      -> taskkill /F /IM <image> (or /PID <pid>)
cpu_stress        -> N worker threads for a bounded duration
network_blackhole -> a temporary Windows Firewall block rule, with rollback
```

- `process_kill` terminates a process by image name or PID.
- `cpu_stress` runs a configured number of busy worker threads for a duration.
- `network_blackhole` creates a scoped firewall rule and removes it during
  rollback.

Use `tumult discover` to inspect the actions in the installed build rather than
relying on a hard-coded provider count.

## Verification model

Command construction and validation are covered by platform-independent tests.
The repository also contains a small `winfault` runner and a Windows demo path
for exercising the operations on a Windows guest. That environment validates
the observable behavior of process termination, CPU load, firewall-rule
creation, and rollback.

The Windows guest proof is not part of the ordinary Linux unit-test matrix and
was not rerun for the 2.16.1 documentation refresh. Treat its saved transcript
as evidence for the recorded environment, not as a claim about every Windows
release or host policy.

## Scope and limitations

These operations depend on Windows facilities such as `taskkill` and `netsh`.
The process and firewall actions require the permissions those tools normally
require. The provider does not claim feature parity with commercial platforms,
Windows Filtering Platform integrations, or ETW-based tooling.

Before using the provider in production, validate the exact Windows version,
permissions, firewall policy, rollback behavior, and observability path in a
non-production target.

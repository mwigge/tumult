---
title: "ADR-006: SSH Transport"
parent: Architecture Decisions
nav_order: 6
---

# ADR-006: SSH as Universal Remote Transport

## Status

Accepted

## Context

Chaos engineering often requires executing actions on remote hosts — killing processes, injecting CPU stress, manipulating network rules. We need a transport layer that works across Linux servers, VMs, and bare-metal infrastructure without requiring agents or daemons on target hosts.

## Decision

Use SSH as the universal remote transport via the `russh` pure-Rust implementation (v0.58+).

### Key choices:

1. **russh over libssh2-sys**: Pure Rust, no C dependencies, cross-compiles cleanly. Async-native with tokio.

2. **Key-based + agent authentication**: Support both direct key files and SSH agent forwarding. No password auth (security hygiene).

3. **Connection pooling**: Reuse SSH sessions across multiple commands to the same host within an experiment run.

4. **File transfer via stdin pipe**: Use `cat > remote_path` over SSH channel rather than SCP protocol — simpler, works universally, no SFTP subsystem needed.

5. **Accept-all host keys by default**: For chaos engineering in trusted environments. Future: known_hosts verification option.

## Consequences

- Single binary still works — no `libssh2.so` or OpenSSL shared library needed
- Remote execution works on any host with an SSH daemon (which is essentially all Linux servers)
- SSH agent integration means no key files need to be deployed alongside Tumult
- The `ExecutionTarget::Ssh` variant in tumult-core maps directly to `SshSession::execute()`

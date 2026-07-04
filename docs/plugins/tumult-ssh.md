---
title: tumult-ssh
parent: Plugins
nav_order: 3
---

# tumult-ssh — SSH Remote Execution

Remote command execution and file transfer over SSH for the Tumult chaos engineering platform.

## Features

- **SSH connection manager** with connection pooling
- **Remote command execution** with stdout/stderr capture and exit code
- **File upload** via SSH channel (no SFTP subsystem required)
- **Key-based authentication** (ed25519, RSA, ECDSA)
- **SSH agent authentication** (ssh-agent / pageant)
- **Host key verification** against `known_hosts` (default), trust-on-first-use, or accept-any
- **Configurable timeouts** for both connection and command execution
- **Native plugin function** `execute` for use in experiments via the `native` provider

## Native `execute` Function

Experiments call the plugin through the `native` provider. The `host_key_policy` argument controls host key verification and defaults to `verify`:

```toon
method[1]:
  - name: restart-service-remote
    activity_type: action
    provider:
      type: native
      plugin: tumult-ssh
      function: execute
      arguments:
        host: db-primary.example.com
        port: 22
        user: ops
        key_file: /home/ops/.ssh/id_ed25519
        command: systemctl restart postgresql
        host_key_policy: verify
```

| Argument | Required | Default | Description |
|----------|----------|---------|-------------|
| `host` | Yes | — | Remote host |
| `user` | Yes | — | SSH user |
| `command` | Yes | — | Command to run |
| `port` | No | `22` | SSH port |
| `key_file` | No | agent auth | Path to a private key; omit to use the SSH agent |
| `host_key_policy` | No | `verify` | `verify`, `trust-on-first-use`, or `accept-any` |

`verify` checks the server key against `known_hosts` and fails with a typed error on unknown or changed keys. `accept-any` is an explicit opt-in for ephemeral targets with unverifiable keys — the old implicit accept-all behaviour is gone. Unknown function names error with the list of available functions.

## Configuration

Activities can target remote hosts via the `ExecutionTarget::Ssh` variant:

```toon
method[1]:
  - name: stress-cpu-remote
    activity_type: action
    provider:
      type: process
      path: stress-ng
      arguments[3]: --cpu, 4, --timeout, 30s
    execution_target:
      type: ssh
      host: db-primary.example.com
      port: 22
      user: ops
      key_path: /home/ops/.ssh/id_ed25519
```

## Authentication

### Key-based

```rust
let config = SshConfig::with_key(
    "db-primary.example.com",
    "ops",
    PathBuf::from("/home/ops/.ssh/id_ed25519"),
);
```

Supported key types: Ed25519, RSA (2048+), ECDSA (P-256, P-384).

### SSH Agent

```rust
let config = SshConfig::with_agent("db-primary.example.com", "ops");
```

Uses the `SSH_AUTH_SOCK` environment variable to connect to a running SSH agent.

## API

### Connect

```rust
let session = SshSession::connect(config).await?;
```

### Execute Command

```rust
let result = session.execute("uname -a").await?;
println!("stdout: {}", result.stdout);
println!("exit code: {}", result.exit_code);
assert!(result.success());
```

### Upload File

```rust
session.upload_file(
    Path::new("scripts/stress.sh"),
    "/tmp/stress.sh",
).await?;
```

### Close

```rust
session.close().await?;
```

## Timeouts

```rust
let config = SshConfig::with_key("host", "user", key_path)
    .connect_timeout(Duration::from_secs(30))  // Connection timeout
    .command_timeout(Duration::from_secs(60));  // Per-command timeout
```

## Error Handling

All SSH operations return `Result<_, SshError>` with these variants:

| Error | Cause |
|-------|-------|
| `ConnectionFailed` | TCP connection or SSH handshake failed |
| `AuthenticationFailed` | Key rejected or agent not available |
| `HostKeyNotFound` | Server key not present in `known_hosts` (policy `verify`) |
| `HostKeyMismatch` | Server key differs from the `known_hosts` entry (possible MITM) |
| `KnownHostsIo` | Failed to read or write the `known_hosts` file |
| `KeyNotFound` | Private key file does not exist |
| `KeyPermissionsTooOpen` | Private key file mode is looser than 0600 |
| `KeyParseError` | Private key file is malformed |
| `ExecutionFailed` | Command could not be started |
| `ChannelError` | SSH channel operation failed |
| `UploadFailed` | File transfer failed |
| `Timeout` | Connection or command timed out |

## Security Notes

### Host Key Verification

Host key verification defaults to **`verify`**: `SshSession::connect` checks the server's key against `known_hosts` and returns a typed `HostKeyNotFound` or `HostKeyMismatch` error when the key is unknown or has changed. Two relaxations are available via `HostKeyPolicy` (or the `host_key_policy` argument of the native `execute` function):

- `trust-on-first-use` — record an unknown key on first connection, then verify it on subsequent connections
- `accept-any` — skip verification entirely; an explicit opt-in for trusted internal networks and ephemeral instances, NOT for production use over untrusted networks

### RSA Key Vulnerability (RUSTSEC-2023-0071)

The `russh` 0.58 dependency tree includes `rsa` 0.10.0-rc.12, which has a known timing side-channel vulnerability ([Marvin Attack](https://rustsec.org/advisories/RUSTSEC-2023-0071), CVSS 5.9 medium). This affects **RSA key authentication only**.

**Mitigation:** Use **Ed25519 keys** (recommended) or **ECDSA keys** instead of RSA keys. Ed25519 is not affected by this vulnerability and is the preferred key type for modern SSH.

```bash
# Generate an Ed25519 key (recommended)
ssh-keygen -t ed25519 -C "tumult-chaos" -f ~/.ssh/tumult_ed25519

# Use it in your experiment
execution_target:
  type: ssh
  host: target-host
  user: ops
  key_path: ~/.ssh/tumult_ed25519
```

No upstream fix is currently available for the RSA crate. This advisory will be resolved when `russh` updates its dependency.

## Implementation Notes

- Uses `russh` 0.58 — pure Rust, no C dependencies
- Async-native with tokio
- File upload uses `cat > path && chmod 755` via SSH channel — no SFTP subsystem needed
- Authentication is bounded by `connect_timeout` to prevent stalls
- Upload operations respect `command_timeout`

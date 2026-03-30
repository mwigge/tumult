# tumult-ssh — SSH Remote Execution

Remote command execution and file transfer over SSH for the Tumult chaos engineering platform.

## Features

- **SSH connection manager** with connection pooling
- **Remote command execution** with stdout/stderr capture and exit code
- **File upload** via SSH channel (no SFTP subsystem required)
- **Key-based authentication** (ed25519, RSA, ECDSA)
- **SSH agent authentication** (ssh-agent / pageant)
- **Configurable timeouts** for both connection and command execution

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
| `KeyNotFound` | Private key file does not exist |
| `KeyParseError` | Private key file is malformed |
| `ExecutionFailed` | Command could not be started |
| `ChannelError` | SSH channel operation failed |
| `ScpFailed` | File transfer failed |
| `Timeout` | Connection or command timed out |

## Implementation Notes

- Uses `russh` 0.58 — pure Rust, no C dependencies
- Async-native with tokio
- Host key verification is currently accept-all (see ADR-006)
- File upload uses `cat > path` via SSH channel — no SFTP subsystem needed

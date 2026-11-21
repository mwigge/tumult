# tumult-ssh

SSH for Tumult: run a command on a remote host. The `tumult-ssh` native
plugin (`SshExecutor`) is built on it. To use it from an experiment, read
the [plugin page](../docs/plugins/tumult-ssh.md).

## Use it from Rust

```rust
use std::path::PathBuf;
use std::time::Duration;
use tumult_ssh::{HostKeyPolicy, SshConfig, SshSession};

let config = SshConfig::with_key("db-primary", "ops", PathBuf::from("/home/ops/.ssh/id_ed25519"))
    .connect_timeout(Duration::from_secs(30))
    .command_timeout(Duration::from_secs(60))
    .host_key_policy(HostKeyPolicy::Verify);
// or: SshConfig::with_agent("db-primary", "ops")

let session = SshSession::connect(config).await?;
let result = session.execute("uname -a").await?;
assert!(result.success()); // exit_code, stdout and stderr are fields
session.close().await?;
```

`with_key` and `with_agent` start with port 22, a 10 second connect
timeout, no command timeout and `HostKeyPolicy::Verify` against
`~/.ssh/known_hosts`. `known_hosts_path` sets another file. The command
timeout is a deadline for the whole exchange.

## Errors

Every call returns `Result<_, SshError>`. The main cases:

| Error | Cause |
|---|---|
| `ConnectionFailed` | The TCP connection or the SSH handshake failed |
| `AuthenticationFailed` | The server refused the key, or no agent answered |
| `HostKeyNotFound` | The key of the server is not in `known_hosts` (policy `Verify`) |
| `HostKeyMismatch` | The key differs from the one in `known_hosts` |
| `KnownHostsIo` | `known_hosts` could not be read or written |
| `KeyNotFound`, `KeyPermissionsTooOpen`, `KeyParseError` | The private key file is missing, readable by others, or not a key |
| `ChannelError`, `ExecutionFailed` | The command could not be started |
| `Timeout` | The connection or the command took too long |

## Build notes

The crate uses `russh` 0.65 with the `aws-lc-rs` crypto backend (it builds
the C library AWS-LC), and without the `rsa` feature. RSA keys therefore
do not load.

# tumult-ssh

SSH remote execution for the Tumult resilience intelligence platform -- run chaos actions and probes on remote hosts.

## Key Types

- `SshSession` -- manages SSH connections via `russh`
- `SshExecutor` -- executes commands on remote targets

## Usage

```rust
use tumult_ssh::{SshConfig, SshSession};

let config = SshConfig::with_agent("host", "user");
let session = SshSession::connect(config).await?;
let output = session.execute("systemctl stop nginx").await?;
```

## More Information

See the [main README](../README.md) for project overview and setup.

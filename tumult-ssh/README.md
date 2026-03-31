# tumult-ssh

SSH remote execution for the Tumult chaos engineering platform -- run chaos actions and probes on remote hosts.

## Key Types

- `SshSession` -- manages SSH connections via `russh`
- `RemoteExecutor` -- executes commands on remote targets

## Usage

```rust
use tumult_ssh::SshSession;

let session = SshSession::connect("host:22", auth).await?;
let output = session.exec("systemctl stop nginx").await?;
```

## More Information

See the [main README](../README.md) for project overview and setup.

//! Tumult SSH — Remote command execution over SSH.
//!
//! Provides SSH connectivity for executing chaos actions and probes
//! on remote hosts. Uses `russh` for pure-Rust SSH2 implementation.
//!
//! # Features
//!
//! - Connection pooling with automatic reconnection
//! - Key-based and SSH agent authentication
//! - Command execution with stdout/stderr capture
//! - SCP file transfer for deploying stress scripts
//!
//! # Authentication methods
//!
//! [`AuthMethod`] supports two strategies:
//!
//! | Variant | Description |
//! |---------|-------------------------------------------------------|
//! | `Key`   | Path to a PEM-encoded private key on disk (optional passphrase, zeroized on drop) |
//! | `Agent` | Delegates to a running `ssh-agent` / pageant process |
//!
//! # Usage
//!
//! Build an [`SshConfig`], open an [`SshSession`], then call
//! [`SshSession::execute`] to run commands on the remote host. The returned
//! [`CommandResult`] captures exit code, stdout, and stderr.

pub mod config;
pub mod error;
pub mod pool;
pub mod session;
pub(crate) mod telemetry;

pub use config::{AuthMethod, HostKeyPolicy, SshConfig};
pub use error::SshError;
pub use pool::SshPool;
pub use session::{CommandResult, SshSession};

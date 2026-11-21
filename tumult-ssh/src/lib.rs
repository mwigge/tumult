//! Tumult SSH — Remote command execution over SSH.
//!
//! Provides SSH connectivity for executing chaos actions and probes
//! on remote hosts. Uses `russh` for pure-Rust SSH2 implementation.
//!
//! # Features
//!
//! - Key-based and SSH agent authentication
//! - Command execution with stdout/stderr capture
//!
//! # Authentication methods
//!
//! [`AuthMethod`] supports two strategies:
//!
//! | Variant | Description |
//! |---------|-------------------------------------------------------|
//! | `Key`   | Path to a PEM-encoded private key on disk (optional passphrase, zeroized on drop) |
//! | `Agent` | Delegates to the running `ssh-agent` whose socket `SSH_AUTH_SOCK` names |
//!
//! # Usage
//!
//! Build an [`SshConfig`], open an [`SshSession`], then call
//! [`SshSession::execute`] to run commands on the remote host. The returned
//! [`CommandResult`] captures exit code, stdout, and stderr.

pub mod config;
pub mod error;
pub mod native;
pub mod session;
pub(crate) mod telemetry;

pub use config::{AuthMethod, HostKeyPolicy, SshConfig};
pub use error::SshError;
pub use native::SshExecutor;
pub use session::{CommandResult, SshSession};

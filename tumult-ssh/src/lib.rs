//! Tumult SSH — Remote command execution over SSH.
//!
//! Provides SSH connectivity for executing chaos actions and probes
//! on remote hosts. Uses `russh` for pure-Rust SSH2 implementation.
//!
//! Features:
//! - Connection pooling with automatic reconnection
//! - Key-based and SSH agent authentication
//! - Command execution with stdout/stderr capture
//! - SCP file transfer for deploying stress scripts

pub mod config;
pub mod error;
pub mod session;

pub use config::{AuthMethod, SshConfig};
pub use error::SshError;
pub use session::{CommandResult, SshSession};

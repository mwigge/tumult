//! Tumult Windows — Windows-native fault injection.
//!
//! `tumult-windows` injects three faults into a Windows host by driving
//! built-in Windows tools (`taskkill`, `netsh`) through
//! [`std::process::Command`], plus a self-contained CPU busy-spin. It uses no
//! raw Win32/WFP calls, so it cross-compiles cleanly to
//! `x86_64-pc-windows-gnu` and its command construction is unit-testable on
//! Linux.
//!
//! # Faults
//!
//! | Function | Effect | Key arguments |
//! |---------------------|-----------------------------------------------|------------------------|
//! | `process_kill`      | `taskkill /F` a process by image name or PID  | `image` \| `pid` |
//! | `cpu_stress`        | Spin N CPU-bound threads for a duration        | `workers`, `duration_secs` |
//! | `network_blackhole` | Add a blocking `netsh` firewall rule (with rollback) | `port` \| `remote_host` |
//!
//! # Design: construction vs. execution
//!
//! [`commands`] is pure — it turns arguments into the exact program + argument
//! vector, with no side effects, and is exhaustively unit-tested on Linux.
//! [`faults`] executes those vectors (Windows-only effect for `taskkill` /
//! `netsh`; cross-platform for the CPU spin). This split is what proves the
//! commands are correct without a Windows box in the loop.
//!
//! # Execution requires a Windows host
//!
//! `process_kill` and `network_blackhole` only take effect where `taskkill` and
//! `netsh` exist; on Linux they return a typed [`error::WindowsError::Spawn`].
//! `cpu_stress` runs anywhere. The plugin is validated live against a real
//! Windows 11 guest.

pub mod commands;
pub mod error;
pub mod faults;
pub mod native;

pub use commands::BlackholeTarget;
pub use error::WindowsError;
pub use faults::{
    cpu_stress, network_blackhole, network_blackhole_rollback, process_kill, BlackholeReport,
    CpuStressReport, ProcessKillReport,
};
pub use native::WindowsExecutor;

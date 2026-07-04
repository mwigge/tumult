//! Fault execution — runs the constructed commands and the CPU busy-spin.
//!
//! The three faults:
//!
//! - [`process_kill`] shells out to `taskkill` (Windows-only effect).
//! - [`network_blackhole`] adds a blocking `netsh` firewall rule and reports
//!   the exact rollback command; [`network_blackhole_rollback`] deletes it.
//! - [`cpu_stress`] is pure Rust — it spins CPU-bound threads and therefore
//!   runs and is observable on any platform, including the Linux CI host.

use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::commands::{
    build_blackhole_add_args, build_blackhole_delete_args, build_taskkill_args, BlackholeTarget,
    NETSH, TASKKILL,
};
use crate::error::WindowsError;

/// Run a Windows tool to completion, returning trimmed stdout on success.
///
/// # Errors
///
/// Returns [`WindowsError::Spawn`] if the program cannot be launched (the
/// expected result on a non-Windows host) and [`WindowsError::CommandFailed`]
/// if it exits non-zero.
fn run(program: &str, args: &[String]) -> Result<String, WindowsError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|source| WindowsError::Spawn {
            program: program.to_string(),
            source,
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(WindowsError::CommandFailed {
            program: program.to_string(),
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

/// Outcome of a [`process_kill`] fault.
#[derive(Debug, Clone)]
pub struct ProcessKillReport {
    /// The program invoked (`taskkill`).
    pub program: String,
    /// The exact arguments passed to it.
    pub args: Vec<String>,
    /// Trimmed stdout from `taskkill`.
    pub stdout: String,
}

impl ProcessKillReport {
    /// Serialise this report to a JSON value.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "program": self.program,
            "args": self.args,
            "stdout": self.stdout,
        })
    }
}

/// Terminate a process by image name or PID via `taskkill /F`.
///
/// Exactly one of `image` or `pid` must be supplied.
///
/// # Errors
///
/// Returns [`WindowsError::InvalidArgument`] for a bad selector,
/// [`WindowsError::Spawn`] if `taskkill` is unavailable (non-Windows host), or
/// [`WindowsError::CommandFailed`] if the process is not found or cannot be
/// killed.
pub fn process_kill(
    image: Option<&str>,
    pid: Option<u32>,
) -> Result<ProcessKillReport, WindowsError> {
    let args = build_taskkill_args(image, pid)?;
    let stdout = run(TASKKILL, &args)?;
    Ok(ProcessKillReport {
        program: TASKKILL.to_string(),
        args,
        stdout,
    })
}

/// Outcome of a [`network_blackhole`] fault.
#[derive(Debug, Clone)]
pub struct BlackholeReport {
    /// The firewall rule that was created.
    pub rule_name: String,
    /// The full `netsh` argument vector that added the rule.
    pub add_args: Vec<String>,
    /// The full `netsh` argument vector that removes it again — the rollback.
    pub rollback_args: Vec<String>,
    /// Trimmed stdout from the add command.
    pub stdout: String,
}

impl BlackholeReport {
    /// The rollback as a single copy-pasteable command line.
    #[must_use]
    pub fn rollback_command(&self) -> String {
        std::iter::once(NETSH.to_string())
            .chain(self.rollback_args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Serialise this report to a JSON value, including the rollback command.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "rule_name": self.rule_name,
            "add_args": self.add_args,
            "rollback_args": self.rollback_args,
            "rollback_command": self.rollback_command(),
            "stdout": self.stdout,
        })
    }
}

/// Block outbound TCP to a port or remote host by adding a `netsh` firewall
/// rule. The returned report carries the exact rollback command.
///
/// # Errors
///
/// Returns [`WindowsError::InvalidArgument`] for a bad target,
/// [`WindowsError::Spawn`] if `netsh` is unavailable (non-Windows host), or
/// [`WindowsError::CommandFailed`] if the rule cannot be added (e.g. the
/// process lacks Administrator rights).
pub fn network_blackhole(target: &BlackholeTarget) -> Result<BlackholeReport, WindowsError> {
    let rule_name = target.rule_name();
    let add_args = build_blackhole_add_args(&rule_name, target);
    let rollback_args = build_blackhole_delete_args(&rule_name);
    let stdout = run(NETSH, &add_args)?;
    Ok(BlackholeReport {
        rule_name,
        add_args,
        rollback_args,
        stdout,
    })
}

/// Roll back a blackhole by deleting the named firewall rule.
///
/// # Errors
///
/// Returns [`WindowsError::Spawn`] if `netsh` is unavailable, or
/// [`WindowsError::CommandFailed`] if the rule cannot be deleted.
pub fn network_blackhole_rollback(rule_name: &str) -> Result<String, WindowsError> {
    let args = build_blackhole_delete_args(rule_name);
    run(NETSH, &args)
}

/// Outcome of a [`cpu_stress`] run.
#[derive(Debug, Clone)]
pub struct CpuStressReport {
    /// Number of busy-spin worker threads that ran.
    pub workers: usize,
    /// Requested spin duration, in seconds.
    pub requested_secs: f64,
    /// Actual wall-clock duration the spin ran, in seconds.
    pub elapsed_secs: f64,
}

impl CpuStressReport {
    /// Serialise this report to a JSON value.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "workers": self.workers,
            "requested_secs": self.requested_secs,
            "elapsed_secs": self.elapsed_secs,
        })
    }
}

/// A reasonable default worker count: the host's parallelism, or 2 if it cannot
/// be determined.
#[must_use]
pub fn default_workers() -> usize {
    thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get)
}

/// Drive CPU load by spinning `workers` CPU-bound threads for `duration`.
///
/// This is self-contained: it needs no external stress tool, and because it is
/// pure Rust it runs on every platform, so its effect is observable via CPU
/// metrics on the Windows guest and its behaviour is unit-testable on Linux.
/// `workers` is clamped to at least 1.
#[must_use]
pub fn cpu_stress(workers: usize, duration: Duration) -> CpuStressReport {
    let workers = workers.max(1);
    let start = Instant::now();

    let handles: Vec<_> = (0..workers)
        .map(|_| {
            thread::spawn(move || {
                let mut acc: u64 = 0;
                while start.elapsed() < duration {
                    // A dependency chain the optimiser cannot elide, so the
                    // thread genuinely burns a core rather than sleeping.
                    acc = std::hint::black_box(
                        acc.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1),
                    );
                }
                acc
            })
        })
        .collect();

    for handle in handles {
        let _ = handle.join();
    }

    CpuStressReport {
        workers,
        requested_secs: duration.as_secs_f64(),
        elapsed_secs: start.elapsed().as_secs_f64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_workers_is_at_least_one() {
        assert!(default_workers() >= 1);
    }

    #[test]
    fn cpu_stress_spins_for_about_the_requested_duration() {
        let requested = Duration::from_millis(250);
        let report = cpu_stress(2, requested);
        assert_eq!(report.workers, 2);
        // It must actually spin for at least the requested time, and not run
        // away far beyond it.
        assert!(
            report.elapsed_secs >= 0.24,
            "spun too briefly: {}s",
            report.elapsed_secs
        );
        assert!(
            report.elapsed_secs < 5.0,
            "spun far too long: {}s",
            report.elapsed_secs
        );
    }

    #[test]
    fn cpu_stress_clamps_zero_workers_to_one() {
        let report = cpu_stress(0, Duration::from_millis(50));
        assert_eq!(report.workers, 1);
    }

    #[test]
    fn process_kill_on_non_windows_reports_spawn_failure() {
        // On the Linux CI host `taskkill` is absent, so this must surface a
        // Spawn error rather than panicking — proving the execution path is
        // reached and errors are typed, without needing Windows.
        if cfg!(not(windows)) {
            let err = process_kill(Some("nonexistent.exe"), None).unwrap_err();
            assert!(matches!(err, WindowsError::Spawn { .. }));
        }
    }

    #[test]
    fn blackhole_report_rollback_command_round_trips() {
        let target = BlackholeTarget::Port(8080);
        let rule_name = target.rule_name();
        let report = BlackholeReport {
            rule_name: rule_name.clone(),
            add_args: build_blackhole_add_args(&rule_name, &target),
            rollback_args: build_blackhole_delete_args(&rule_name),
            stdout: String::new(),
        };
        assert_eq!(
            report.rollback_command(),
            "netsh advfirewall firewall delete rule name=tumult-blackhole-port-8080"
        );
    }
}

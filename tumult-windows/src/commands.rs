//! Pure command construction — no process is spawned here.
//!
//! Every function in this module turns validated arguments into the exact
//! program name and argument vector that [`crate::faults`] later hands to
//! `std::process::Command`. Keeping construction free of side effects is what
//! makes the crate's correctness provable on Linux (see the unit tests below):
//! the argument vectors are asserted without a Windows host in the loop, and
//! the same vectors are what run against the real Windows 11 guest.

use crate::error::WindowsError;

/// The Windows process-termination tool.
pub const TASKKILL: &str = "taskkill";

/// The Windows network-shell tool used to drive the firewall.
pub const NETSH: &str = "netsh";

/// Prefix applied to every firewall rule this crate creates, so blackhole
/// rules are recognisable and the rollback can address them by name.
pub const RULE_PREFIX: &str = "tumult-blackhole";

/// Build the argument vector for a `taskkill` invocation.
///
/// Exactly one of `image` (an executable image name such as `notepad.exe`) or
/// `pid` (a numeric process id) must be supplied.
///
/// ```
/// use tumult_windows::commands::build_taskkill_args;
/// let args = build_taskkill_args(Some("notepad.exe"), None).unwrap();
/// assert_eq!(args, ["/F", "/IM", "notepad.exe"]);
/// ```
///
/// # Errors
///
/// Returns [`WindowsError::InvalidArgument`] if neither or both of `image` and
/// `pid` are provided, or if `image` is empty.
pub fn build_taskkill_args(
    image: Option<&str>,
    pid: Option<u32>,
) -> Result<Vec<String>, WindowsError> {
    match (image, pid) {
        (Some(image), None) => {
            if image.trim().is_empty() {
                return Err(WindowsError::invalid_argument("image", "must not be empty"));
            }
            Ok(vec!["/F".into(), "/IM".into(), image.into()])
        }
        (None, Some(pid)) => Ok(vec!["/F".into(), "/PID".into(), pid.to_string()]),
        (Some(_), Some(_)) => Err(WindowsError::invalid_argument(
            "image",
            "`image` and `pid` are mutually exclusive — supply exactly one",
        )),
        (None, None) => Err(WindowsError::invalid_argument(
            "image",
            "one of `image` or `pid` is required",
        )),
    }
}

/// What a network-blackhole fault blocks: a TCP port or a remote host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlackholeTarget {
    /// Block outbound TCP to this remote port.
    Port(u16),
    /// Block outbound TCP to this remote IP or hostname.
    RemoteHost(String),
}

impl BlackholeTarget {
    /// Resolve a [`BlackholeTarget`] from the optional `port` / `remote_host`
    /// arguments. Exactly one must be supplied.
    ///
    /// # Errors
    ///
    /// Returns [`WindowsError::InvalidArgument`] if neither or both are given,
    /// or if `remote_host` is empty.
    pub fn from_args(port: Option<u16>, remote_host: Option<&str>) -> Result<Self, WindowsError> {
        match (port, remote_host) {
            (Some(port), None) => Ok(Self::Port(port)),
            (None, Some(host)) => {
                if host.trim().is_empty() {
                    return Err(WindowsError::invalid_argument(
                        "remote_host",
                        "must not be empty",
                    ));
                }
                Ok(Self::RemoteHost(host.to_string()))
            }
            (Some(_), Some(_)) => Err(WindowsError::invalid_argument(
                "port",
                "`port` and `remote_host` are mutually exclusive — supply exactly one",
            )),
            (None, None) => Err(WindowsError::invalid_argument(
                "port",
                "one of `port` or `remote_host` is required",
            )),
        }
    }

    /// The deterministic firewall-rule name for this target.
    ///
    /// The name is stable given the same target, so the rollback can delete the
    /// exact rule the fault created without tracking extra state.
    #[must_use]
    pub fn rule_name(&self) -> String {
        match self {
            Self::Port(port) => format!("{RULE_PREFIX}-port-{port}"),
            Self::RemoteHost(host) => format!("{RULE_PREFIX}-host-{host}"),
        }
    }

    /// The `netsh` match clause selecting this target
    /// (`remoteport=443` or `remoteip=10.0.0.5`).
    fn match_clause(&self) -> String {
        match self {
            Self::Port(port) => format!("remoteport={port}"),
            Self::RemoteHost(host) => format!("remoteip={host}"),
        }
    }
}

/// Build the argument vector that adds the blocking firewall rule.
///
/// Mirrors
/// `netsh advfirewall firewall add rule name=<n> dir=out action=block
/// remoteport=<port> protocol=TCP`.
#[must_use]
pub fn build_blackhole_add_args(rule_name: &str, target: &BlackholeTarget) -> Vec<String> {
    vec![
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={rule_name}"),
        "dir=out".into(),
        "action=block".into(),
        "protocol=TCP".into(),
        target.match_clause(),
    ]
}

/// Build the argument vector that deletes the blocking firewall rule — the
/// blackhole rollback.
///
/// Mirrors `netsh advfirewall firewall delete rule name=<n>`.
#[must_use]
pub fn build_blackhole_delete_args(rule_name: &str) -> Vec<String> {
    vec![
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={rule_name}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taskkill_by_image() {
        let args = build_taskkill_args(Some("notepad.exe"), None).unwrap();
        assert_eq!(args, ["/F", "/IM", "notepad.exe"]);
    }

    #[test]
    fn taskkill_by_pid() {
        let args = build_taskkill_args(None, Some(4321)).unwrap();
        assert_eq!(args, ["/F", "/PID", "4321"]);
    }

    #[test]
    fn taskkill_requires_exactly_one_selector() {
        assert!(build_taskkill_args(None, None).is_err());
        assert!(build_taskkill_args(Some("notepad.exe"), Some(4321)).is_err());
    }

    #[test]
    fn taskkill_rejects_empty_image() {
        let err = build_taskkill_args(Some("   "), None).unwrap_err();
        assert!(err.to_string().contains("image"));
    }

    #[test]
    fn blackhole_target_from_args_is_exclusive() {
        assert_eq!(
            BlackholeTarget::from_args(Some(443), None).unwrap(),
            BlackholeTarget::Port(443)
        );
        assert_eq!(
            BlackholeTarget::from_args(None, Some("10.0.0.5")).unwrap(),
            BlackholeTarget::RemoteHost("10.0.0.5".into())
        );
        assert!(BlackholeTarget::from_args(None, None).is_err());
        assert!(BlackholeTarget::from_args(Some(443), Some("10.0.0.5")).is_err());
    }

    #[test]
    fn blackhole_rule_names_are_deterministic() {
        assert_eq!(
            BlackholeTarget::Port(443).rule_name(),
            "tumult-blackhole-port-443"
        );
        assert_eq!(
            BlackholeTarget::RemoteHost("10.0.0.5".into()).rule_name(),
            "tumult-blackhole-host-10.0.0.5"
        );
    }

    #[test]
    fn blackhole_add_rule_string_for_port() {
        let target = BlackholeTarget::Port(443);
        let name = target.rule_name();
        let args = build_blackhole_add_args(&name, &target);
        assert_eq!(
            args,
            [
                "advfirewall",
                "firewall",
                "add",
                "rule",
                "name=tumult-blackhole-port-443",
                "dir=out",
                "action=block",
                "protocol=TCP",
                "remoteport=443",
            ]
        );
    }

    #[test]
    fn blackhole_add_rule_string_for_remote_host() {
        let target = BlackholeTarget::RemoteHost("10.0.0.5".into());
        let name = target.rule_name();
        let args = build_blackhole_add_args(&name, &target);
        assert!(args.contains(&"remoteip=10.0.0.5".to_string()));
        assert!(args.contains(&"name=tumult-blackhole-host-10.0.0.5".to_string()));
        assert!(args.contains(&"action=block".to_string()));
    }

    #[test]
    fn blackhole_delete_rule_string() {
        let args = build_blackhole_delete_args("tumult-blackhole-port-443");
        assert_eq!(
            args,
            [
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                "name=tumult-blackhole-port-443",
            ]
        );
    }
}

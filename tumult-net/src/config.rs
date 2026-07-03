//! Fault configuration and the cross-process proxy specification.
//!
//! Because every native action runs in its own short-lived process (the CLI
//! dispatches with `block_in_place` + `block_on`), a long-running proxy fault
//! must persist its configuration in the operating system. [`ProxySpec`]
//! serialises the listen/upstream addresses, the [`FaultProfile`], and the
//! pidfile path into a `--flag value` argv vector that the `tumult-net-proxyd`
//! daemon reconstructs on startup.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::error::NetError;

/// A bundle of directional TCP faults applied by the proxy.
///
/// A value of `0` (or `0.0`) disables the corresponding fault, matching the
/// pass-through semantics of the underlying `tokio-netem` adapters.
#[derive(Debug, Clone, PartialEq)]
pub struct FaultProfile {
    /// Base one-way latency added to every write, in milliseconds.
    pub delay_ms: u64,
    /// Maximum extra latency drawn deterministically from `seed`, in milliseconds.
    pub jitter_ms: u64,
    /// Egress throughput limit in bytes/second (`0` = unlimited).
    pub rate_bps: usize,
    /// Fixed write-segment size in bytes for fragmentation (`0` = no slicing).
    pub slice_bytes: usize,
    /// Per-byte corruption probability in `0.0..=1.0` (`0.0` = none).
    pub corrupt_prob: f64,
    /// Per-write hard-close probability in `0.0..=1.0` (`0.0` = none).
    pub terminate_prob: f64,
    /// Seed governing the reproducible fault schedule (jitter + corruption RNG).
    pub seed: u64,
}

impl Default for FaultProfile {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            jitter_ms: 0,
            rate_bps: 0,
            slice_bytes: 0,
            corrupt_prob: 0.0,
            terminate_prob: 0.0,
            seed: 0,
        }
    }
}

impl FaultProfile {
    /// Validate that probability fields fall inside `0.0..=1.0`.
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InvalidConfig`] when `corrupt_prob` or
    /// `terminate_prob` is outside the closed unit interval or not finite.
    pub fn validate(&self) -> Result<(), NetError> {
        for (field, value) in [
            ("corrupt_prob", self.corrupt_prob),
            ("terminate_prob", self.terminate_prob),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(NetError::invalid(
                    field,
                    format!("probability must be within 0.0..=1.0, got {value}"),
                ));
            }
        }
        Ok(())
    }

    /// Derive the 32-byte seed handed to the `tokio-netem` corruption and
    /// termination RNGs so their output is reproducible across runs.
    #[must_use]
    pub fn seed_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        let mut state = self.seed;
        for chunk in out.chunks_mut(8) {
            // splitmix64 — a fast, well-distributed deterministic expansion.
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes());
        }
        out
    }

    /// Compute the effective latency (base + deterministic jitter) for the
    /// proxy's initial delay knob.
    #[must_use]
    pub fn effective_delay(&self) -> Duration {
        let extra = if self.jitter_ms == 0 {
            0
        } else {
            // Deterministic offset in `0..=jitter_ms` derived from the seed.
            let mut z = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z ^= z >> 27;
            z % (self.jitter_ms + 1)
        };
        Duration::from_millis(self.delay_ms + extra)
    }
}

/// Everything the detached daemon needs to run a faulted proxy.
#[derive(Debug, Clone, PartialEq)]
pub struct ProxySpec {
    /// Downstream address the proxy binds and accepts client connections on.
    pub listen: SocketAddr,
    /// Upstream address the proxy dials for every accepted connection.
    pub upstream: SocketAddr,
    /// The directional faults applied to forwarded traffic.
    pub profile: FaultProfile,
    /// The pidfile the daemon and its rollback use to discover the process.
    pub pidfile: PathBuf,
}

impl ProxySpec {
    /// Render the spec as a `--flag value` argv vector for the daemon.
    #[must_use]
    pub fn to_argv(&self) -> Vec<String> {
        vec![
            "--listen".into(),
            self.listen.to_string(),
            "--upstream".into(),
            self.upstream.to_string(),
            "--delay-ms".into(),
            self.profile.delay_ms.to_string(),
            "--jitter-ms".into(),
            self.profile.jitter_ms.to_string(),
            "--rate-bps".into(),
            self.profile.rate_bps.to_string(),
            "--slice-bytes".into(),
            self.profile.slice_bytes.to_string(),
            "--corrupt-prob".into(),
            self.profile.corrupt_prob.to_string(),
            "--terminate-prob".into(),
            self.profile.terminate_prob.to_string(),
            "--seed".into(),
            self.profile.seed.to_string(),
            "--pidfile".into(),
            self.pidfile.display().to_string(),
        ]
    }

    /// Reconstruct a spec from a daemon argv slice produced by [`Self::to_argv`].
    ///
    /// # Errors
    ///
    /// Returns [`NetError::InvalidConfig`] if a required flag is missing, an
    /// unknown flag appears, or a value fails to parse.
    pub fn from_argv(args: &[String]) -> Result<Self, NetError> {
        let mut listen: Option<SocketAddr> = None;
        let mut upstream: Option<SocketAddr> = None;
        let mut pidfile: Option<PathBuf> = None;
        let mut profile = FaultProfile::default();

        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let value = it
                .next()
                .ok_or_else(|| NetError::invalid("argv", format!("flag `{flag}` needs a value")))?;
            match flag.as_str() {
                "--listen" => listen = Some(parse_field("listen", value)?),
                "--upstream" => upstream = Some(parse_field("upstream", value)?),
                "--delay-ms" => profile.delay_ms = parse_field("delay_ms", value)?,
                "--jitter-ms" => profile.jitter_ms = parse_field("jitter_ms", value)?,
                "--rate-bps" => profile.rate_bps = parse_field("rate_bps", value)?,
                "--slice-bytes" => profile.slice_bytes = parse_field("slice_bytes", value)?,
                "--corrupt-prob" => profile.corrupt_prob = parse_field("corrupt_prob", value)?,
                "--terminate-prob" => {
                    profile.terminate_prob = parse_field("terminate_prob", value)?;
                }
                "--seed" => profile.seed = parse_field("seed", value)?,
                "--pidfile" => pidfile = Some(PathBuf::from(value)),
                other => {
                    return Err(NetError::invalid(
                        "argv",
                        format!("unknown flag `{other}`"),
                    ))
                }
            }
        }

        profile.validate()?;
        Ok(Self {
            listen: listen.ok_or_else(|| NetError::invalid("listen", "flag --listen is required"))?,
            upstream: upstream
                .ok_or_else(|| NetError::invalid("upstream", "flag --upstream is required"))?,
            profile,
            pidfile: pidfile
                .ok_or_else(|| NetError::invalid("pidfile", "flag --pidfile is required"))?,
        })
    }
}

fn parse_field<T>(field: &'static str, raw: &str) -> Result<T, NetError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    raw.parse::<T>()
        .map_err(|e| NetError::invalid(field, format!("cannot parse `{raw}`: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_all_passthrough() {
        let p = FaultProfile::default();
        assert!(p.validate().is_ok());
        assert_eq!(p.effective_delay(), Duration::ZERO);
    }

    #[test]
    fn out_of_range_probability_is_rejected() {
        let p = FaultProfile {
            corrupt_prob: 1.5,
            ..FaultProfile::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn seed_bytes_are_deterministic() {
        let a = FaultProfile {
            seed: 42,
            ..FaultProfile::default()
        };
        let b = FaultProfile {
            seed: 42,
            ..FaultProfile::default()
        };
        assert_eq!(a.seed_bytes(), b.seed_bytes());
        let c = FaultProfile {
            seed: 43,
            ..FaultProfile::default()
        };
        assert_ne!(a.seed_bytes(), c.seed_bytes());
    }

    #[test]
    fn jitter_is_bounded_and_deterministic() {
        let p = FaultProfile {
            delay_ms: 10,
            jitter_ms: 5,
            seed: 7,
            ..FaultProfile::default()
        };
        let d = p.effective_delay();
        assert!(d >= Duration::from_millis(10));
        assert!(d <= Duration::from_millis(15));
        assert_eq!(d, p.effective_delay());
    }

    #[test]
    fn proxy_spec_argv_round_trips() {
        let spec = ProxySpec {
            listen: "127.0.0.1:8080".parse().unwrap(),
            upstream: "10.0.0.1:5432".parse().unwrap(),
            profile: FaultProfile {
                delay_ms: 25,
                jitter_ms: 3,
                rate_bps: 1024,
                slice_bytes: 64,
                corrupt_prob: 0.01,
                terminate_prob: 0.0,
                seed: 99,
            },
            pidfile: PathBuf::from("/tmp/tumult-net-8080.pid"),
        };
        let argv = spec.to_argv();
        let back = ProxySpec::from_argv(&argv).expect("round trip");
        assert_eq!(spec, back);
    }

    #[test]
    fn from_argv_rejects_unknown_flag() {
        let argv = vec!["--bogus".to_string(), "x".to_string()];
        assert!(ProxySpec::from_argv(&argv).is_err());
    }
}

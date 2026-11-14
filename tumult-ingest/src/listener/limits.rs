//! How many connections the gRPC listener keeps open, and for how long.

use std::time::Duration;

/// How many connections the gRPC listener keeps open, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLimits {
    /// Open connections in total (`KRONIKA_GRPC_MAX_CONNECTIONS`).
    pub max_connections: usize,
    /// Open connections from one peer, an IPv4 address or an IPv6 /64
    /// (`KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER`).
    pub max_per_peer: usize,
    /// Open connections from one IPv6 /48, the block a site is usually
    /// given, whatever /64 of it they come from
    /// (`KRONIKA_GRPC_MAX_CONNECTIONS_PER_IPV6_SITE`).
    pub max_per_ipv6_site: usize,
    /// How long a connection may take to complete the TLS handshake and
    /// send the HTTP/2 preface (`KRONIKA_GRPC_HANDSHAKE_TIMEOUT_SECS`).
    pub handshake_timeout: Duration,
    /// With an ingest token configured: how long a connection may take,
    /// from the moment it is accepted, to make its first authorised call
    /// (`KRONIKA_GRPC_AUTH_TIMEOUT_SECS`).
    pub auth_timeout: Duration,
    /// The age a connection may reach
    /// (`KRONIKA_GRPC_MAX_CONNECTION_AGE_SECS`).
    pub max_connection_age: Duration,
    /// How long after that the connection is cut, whatever is in flight
    /// (`KRONIKA_GRPC_MAX_CONNECTION_AGE_GRACE_SECS`).
    pub max_connection_age_grace: Duration,
}

/// The most connections a configuration can ask for.
pub(super) const MAX_CONNECTIONS_CEILING: usize = 65_536;
/// The /64 allowances one IPv6 /48 gets unless the configuration says
/// otherwise.
const PEERS_PER_IPV6_SITE: usize = 4;
/// The longest handshake timeout a configuration can ask for, in seconds.
const MAX_HANDSHAKE_TIMEOUT_SECS: usize = 300;
/// The longest a configuration can let a connection wait for its first
/// authorised call, in seconds.
const MAX_AUTH_TIMEOUT_SECS: usize = 300;
/// The longest connection age a configuration can ask for: a week.
const MAX_CONNECTION_AGE_SECS: usize = 7 * 24 * 60 * 60;
/// The longest grace a configuration can ask for: an hour.
const MAX_CONNECTION_AGE_GRACE_SECS: usize = 60 * 60;

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            max_per_peer: 64,
            max_per_ipv6_site: 64 * PEERS_PER_IPV6_SITE,
            handshake_timeout: Duration::from_secs(10),
            auth_timeout: Duration::from_secs(10),
            max_connection_age: Duration::from_secs(30 * 60),
            max_connection_age_grace: Duration::from_secs(10 * 60),
        }
    }
}

impl ConnectionLimits {
    /// The defaults overridden by the `KRONIKA_GRPC_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] with the variable lookup injected.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let count = |variable: &str, default: usize, max: usize| {
            bounded(lookup(variable), variable, default, max)
        };
        let seconds = |variable: &str, default: Duration, max: usize| {
            let default = usize::try_from(default.as_secs()).unwrap_or(max);
            let secs = bounded(lookup(variable), variable, default, max);
            Duration::from_secs(u64::try_from(secs).unwrap_or(u64::MAX))
        };
        let max_connections = count(
            "KRONIKA_GRPC_MAX_CONNECTIONS",
            d.max_connections,
            MAX_CONNECTIONS_CEILING,
        );
        let max_per_peer = count(
            "KRONIKA_GRPC_MAX_CONNECTIONS_PER_PEER",
            d.max_per_peer.min(max_connections),
            max_connections,
        );
        Self {
            max_connections,
            max_per_peer,
            max_per_ipv6_site: count(
                "KRONIKA_GRPC_MAX_CONNECTIONS_PER_IPV6_SITE",
                max_per_peer
                    .saturating_mul(PEERS_PER_IPV6_SITE)
                    .min(max_connections),
                max_connections,
            ),
            handshake_timeout: seconds(
                "KRONIKA_GRPC_HANDSHAKE_TIMEOUT_SECS",
                d.handshake_timeout,
                MAX_HANDSHAKE_TIMEOUT_SECS,
            ),
            auth_timeout: seconds(
                "KRONIKA_GRPC_AUTH_TIMEOUT_SECS",
                d.auth_timeout,
                MAX_AUTH_TIMEOUT_SECS,
            ),
            max_connection_age: seconds(
                "KRONIKA_GRPC_MAX_CONNECTION_AGE_SECS",
                d.max_connection_age,
                MAX_CONNECTION_AGE_SECS,
            ),
            max_connection_age_grace: seconds(
                "KRONIKA_GRPC_MAX_CONNECTION_AGE_GRACE_SECS",
                d.max_connection_age_grace,
                MAX_CONNECTION_AGE_GRACE_SECS,
            ),
        }
    }
}

/// A positive integer of at most `max`, or `default` when `raw` is not one.
fn bounded(raw: Option<String>, variable: &str, default: usize, max: usize) -> usize {
    let Some(raw) = raw else {
        return default;
    };
    match raw.trim().parse::<usize>() {
        Ok(v) if v > max => {
            tracing::warn!(
                variable,
                max,
                "connection limit above its maximum, using the maximum"
            );
            max
        }
        Ok(v) if v > 0 => v,
        _ => {
            tracing::warn!(
                variable,
                default,
                "ignoring connection limit that is not a positive integer"
            );
            default
        }
    }
}

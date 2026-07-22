//! Shared HTTP client construction for the provider connectors.
//!
//! Every connector builds its `reqwest` client here so a hung provider API
//! fails the experiment call with a timeout error instead of hanging the
//! experiment forever.

use std::time::Duration;

/// Timeout for establishing the TCP/TLS connection to a provider API.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Overall timeout for one provider API request, from connect to body read.
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

/// Build a `reqwest` client with explicit connect and overall timeouts.
///
/// # Panics
///
/// Panics only if the platform TLS backend is so broken that the default
/// client cannot be built at all — in which case no request could ever have
/// succeeded anyway.
pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("default reqwest client with timeouts must build")
}

#[cfg(test)]
mod tests {
    #[test]
    fn client_builds_with_timeouts() {
        let _ = super::client();
    }
}

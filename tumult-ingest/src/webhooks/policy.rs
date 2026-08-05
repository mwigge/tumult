//! Webhook URL policy (SSRF guard) and payload signing — see the parent
//! module docs for the delivery semantics.

use hmac::{Hmac, Mac};

fn env_flag(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Validate a webhook URL against the SSRF policy (env-flag driven; see the
/// module docs).
///
/// # Errors
/// Returns a human-readable reason when the URL is not acceptable.
pub fn validate_webhook_url(url: &str) -> Result<(), String> {
    validate_url_with(
        url,
        env_flag("TUMULTD_WEBHOOK_ALLOW_INSECURE"),
        env_flag("TUMULTD_WEBHOOK_ALLOW_LOCAL"),
    )
}

fn validate_url_with(url: &str, allow_insecure: bool, allow_local: bool) -> Result<(), String> {
    if url.chars().count() > 2_000 {
        return Err("url too long".into());
    }
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if allow_insecure => {}
        other => {
            return Err(format!(
                "scheme {other:?} not allowed: webhooks must be https (TUMULTD_WEBHOOK_ALLOW_INSECURE=1 permits http)"
            ));
        }
    }
    let host = parsed.host_str().ok_or("url must name a host")?;
    // url::Url keeps IPv6 brackets in host_str ("[::1]"); strip them before
    // the IP-literal check or v6 literals sail through as "hostnames".
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        if !allow_local && is_local(ip) {
            return Err(
                "loopback, private and link-local addresses are not allowed (TUMULTD_WEBHOOK_ALLOW_LOCAL=1 permits them)"
                    .into(),
            );
        }
    }
    Ok(())
}

/// Loopback, unspecified, private, or link-local address.
fn is_local(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_unspecified() || v4.is_private() || v4.is_link_local()
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            let seg = v6.segments()[0];
            // fe80::/10 link-local; fc00::/7 unique-local.
            (seg & 0xffc0) == 0xfe80 || (seg & 0xfe00) == 0xfc00
        }
    }
}

/// Lowercase hex HMAC-SHA256 of `msg` under `key` — the value behind
/// `X-Tumult-Signature: sha256=<hex>`.
#[must_use]
pub fn hmac_sha256_hex(key: &str, msg: &str) -> String {
    let mut mac = <Hmac<sha2::Sha256> as Mac>::new_from_slice(key.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(msg.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from(b"0123456789abcdef"[usize::from(b >> 4)]));
        out.push(char::from(b"0123456789abcdef"[usize::from(b & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy() {
        // https hostnames are always fine.
        assert!(validate_url_with("https://hooks.example.com/x", false, false).is_ok());
        // http needs the insecure opt-in.
        assert!(validate_url_with("http://hooks.example.com/x", false, false).is_err());
        assert!(validate_url_with("http://hooks.example.com/x", true, false).is_ok());
        // Other schemes are never allowed.
        assert!(validate_url_with("ftp://example.com/x", true, true).is_err());
        // Local IPs need the local opt-in.
        for local in [
            "https://127.0.0.1:8080/x",
            "https://[::1]/x",
            "https://169.254.169.254/latest",
            "https://192.168.1.10/x",
            "https://10.0.0.4/x",
            "https://[fe80::1]/x",
            "https://[fd00::1]/x",
        ] {
            assert!(validate_url_with(local, false, false).is_err(), "{local}");
            assert!(validate_url_with(local, false, true).is_ok(), "{local}");
        }
        // Garbage is rejected.
        assert!(validate_url_with("not-a-url", false, false).is_err());
    }

    #[test]
    fn hmac_matches_rfc4231_case2() {
        // RFC 4231 test case 2.
        assert_eq!(
            hmac_sha256_hex("Jefe", "what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}

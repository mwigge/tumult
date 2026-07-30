//! Shared authentication primitives for the Tumult platform.
//!
//! This crate is the single source of truth for:
//!
//! - [`Role`] — the RBAC role enum (`viewer < operator < approver < admin`),
//!   parsed fail-closed from configuration.
//! - [`hash_password`] / [`verify_password`] — argon2id password hashing with
//!   the OWASP-recommended parameters (m = 19 MiB, t = 2, p = 1).
//! - [`dummy_password_hash`] — a precomputed valid PHC-string hash used for
//!   timing equalization when a username does not exist.
//! - [`new_session_id`] / [`new_token`] / [`new_password`] — cryptographically
//!   random identifier and credential generation.
//! - [`sha256_hex`] and [`constant_time_eq`] — comparison helpers.
//! - [`host_is_loopback`] — the bind-policy check shared by every server that
//!   refuses a network-exposed bind without configured authentication.

use std::sync::LazyLock;

use argon2::{PasswordHash, PasswordHasher, PasswordVerifier};
use sha2::Digest;
use subtle::ConstantTimeEq;

/// RBAC roles, ordered `viewer < operator < approver < admin` (derived `Ord`).
///
/// The ordering is meaningful: a principal may perform an action when its
/// role is `>=` the action's required role. `Viewer` is declared first so the
/// derived `Ord` yields the ordering above.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// May call read-only operations only.
    Viewer,
    /// May call every operation (read-only + fault injection / execution).
    Operator,
    /// Operator plus approval authority for gated actions.
    Approver,
    /// Unrestricted administrative access.
    Admin,
}

impl Role {
    /// Parse a role name, case-insensitively and ignoring surrounding
    /// whitespace.
    ///
    /// Returns `None` for any unrecognised value — a fail-closed decision: an
    /// unknown role is rejected at config-load time, never elevated.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "approver" => Some(Self::Approver),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }

    /// The canonical lower-case name for this role.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Approver => "approver",
            Self::Admin => "admin",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An authentication-layer error (hashing failure, invalid parameters).
///
/// Carries a human-readable message; never contains key material.
#[derive(Debug)]
pub struct AuthError(pub String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthError {}

/// OWASP-recommended argon2id memory cost: 19 MiB (19456 KiB).
const ARGON2_MEMORY_KIB: u32 = 19_456;
/// OWASP-recommended argon2id iteration count.
const ARGON2_ITERATIONS: u32 = 2;
/// OWASP-recommended argon2id parallelism.
const ARGON2_PARALLELISM: u32 = 1;

/// The argon2id hasher configured with the OWASP parameters.
fn owasp_argon2() -> Result<argon2::Argon2<'static>, AuthError> {
    let params = argon2::Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        None,
    )
    .map_err(|e| AuthError(format!("invalid argon2 parameters: {e}")))?;
    Ok(argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        params,
    ))
}

/// Hash a password into an argon2id PHC string (`$argon2id$v=19$m=19456,...`)
/// with a fresh random salt.
///
/// # Errors
///
/// Returns [`AuthError`] if the argon2 parameters or the hashing operation
/// fail (both are static configurations, so this is not expected in practice).
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let hash = owasp_argon2()?
        .hash_password(password.as_bytes())
        .map_err(|e| AuthError(format!("password hashing failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verify a password against an argon2 PHC-string hash.
///
/// Returns `false` on a malformed hash or a mismatch — this function never
/// panics and never reveals *why* verification failed. The algorithm and
/// parameters are taken from the hash itself, so hashes produced with other
/// argon2 variants/parameters still verify.
#[must_use]
pub fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// A valid PHC-string hash of a random throwaway password, for timing
/// equalization when a username does not exist: verify the presented password
/// against this hash and discard the result, so a missing user costs the same
/// argon2 work as a real one.
///
/// Computed once on first use and cached for the process lifetime.
#[must_use]
pub fn dummy_password_hash() -> &'static str {
    static DUMMY: LazyLock<String> = LazyLock::new(|| {
        hash_password(&new_password()).expect("argon2id with OWASP parameters cannot fail")
    });
    DUMMY.as_str()
}

/// Lowercase hex encoding of `bytes` (two chars per byte, no separators).
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

/// Fill `buf` with cryptographically random bytes from the OS RNG.
///
/// Panics only if the OS RNG fails, which indicates a fatally broken system.
fn fill_random(buf: &mut [u8]) {
    getrandom::fill(buf).expect("OS RNG failure: cannot generate random bytes");
}

/// 32 cryptographically random bytes from the OS RNG.
fn random_bytes() -> [u8; 32] {
    let mut buf = [0u8; 32];
    fill_random(&mut buf);
    buf
}

/// A new opaque session id: 64 lowercase hex characters (32 random bytes).
///
/// # Panics
///
/// Panics only if the OS RNG fails, which indicates a fatally broken system.
#[must_use]
pub fn new_session_id() -> String {
    hex_encode(&random_bytes())
}

/// A new API token: `kro_` followed by 64 lowercase hex characters
/// (32 random bytes). The prefix makes tokens recognisable in logs and
/// secret-scanning rules.
///
/// # Panics
///
/// Panics only if the OS RNG fails, which indicates a fatally broken system.
#[must_use]
pub fn new_token() -> String {
    format!("kro_{}", hex_encode(&random_bytes()))
}

/// A new one-time bootstrap password: 24 cryptographically random characters
/// from `[A-Za-z0-9]`. Rejection sampling avoids modulo bias.
///
/// # Panics
///
/// Panics only if the OS RNG fails, which indicates a fatally broken system.
#[must_use]
pub fn new_password() -> String {
    const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    // Largest multiple of 62 that fits in a byte: bytes >= 248 are rejected.
    const REJECT_AT: u8 = 248;
    let mut out = String::with_capacity(24);
    while out.len() < 24 {
        let mut buf = [0u8; 32];
        fill_random(&mut buf);
        for &b in &buf {
            if b < REJECT_AT {
                out.push(char::from(ALPHABET[usize::from(b % 62)]));
                if out.len() == 24 {
                    break;
                }
            }
        }
    }
    out
}

/// The lowercase hex SHA-256 digest of `s`.
#[must_use]
pub fn sha256_hex(s: &str) -> String {
    hex_encode(&sha2::Sha256::digest(s.as_bytes()))
}

/// Constant-time string comparison.
///
/// Equal-length inputs are compared byte-by-byte with
/// [`subtle::ConstantTimeEq`], so the comparison time reveals nothing about
/// where they differ. When the lengths differ the strings cannot be equal;
/// to keep the timing shape length-independent, both inputs are first hashed
/// with SHA-256 and the fixed-length digests are compared instead (the result
/// is still `false`). Hashing time itself is linear in the input length —
/// this function equalizes the *comparison*, not the hashing, which is the
/// standard length-safe constant-time-compare construction.
#[must_use]
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() == b.len() {
        a.as_bytes().ct_eq(b.as_bytes()).into()
    } else {
        let _ = sha2::Sha256::digest(a.as_bytes()).ct_eq(&sha2::Sha256::digest(b.as_bytes()));
        false
    }
}

/// Whether a bind host is loopback-only (safe to serve without a token).
///
/// Recognises the common loopback spellings: surrounding whitespace and
/// `[...]` brackets are trimmed, `localhost` is accepted by name, and
/// anything else is parsed as an IP address and checked with
/// [`std::net::IpAddr::is_loopback`]. Anything else (including `0.0.0.0`,
/// `::`, or a routable address) is treated as network-exposed, which servers
/// refuse to bind without configured authentication.
#[must_use]
pub fn host_is_loopback(host: &str) -> bool {
    match host.trim().trim_matches(|c| c == '[' || c == ']') {
        "localhost" => true,
        h => h
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Role ordering & parsing ───────────────────────────────────

    #[test]
    fn roles_are_ordered_viewer_to_admin() {
        assert!(Role::Viewer < Role::Operator);
        assert!(Role::Operator < Role::Approver);
        assert!(Role::Approver < Role::Admin);
        // Every higher role satisfies every lower requirement.
        for higher in [Role::Operator, Role::Approver, Role::Admin] {
            assert!(higher >= Role::Viewer);
        }
        for higher in [Role::Approver, Role::Admin] {
            assert!(higher >= Role::Operator);
        }
        assert!(Role::Admin >= Role::Approver);
        assert!(Role::Viewer < Role::Admin);
    }

    #[test]
    fn role_parse_accepts_all_four_case_insensitively_and_trimmed() {
        assert_eq!(Role::parse("viewer"), Some(Role::Viewer));
        assert_eq!(Role::parse("OPERATOR"), Some(Role::Operator));
        assert_eq!(Role::parse(" Approver "), Some(Role::Approver));
        assert_eq!(Role::parse("aDmIn"), Some(Role::Admin));
    }

    #[test]
    fn role_parse_fails_closed_on_unknown_values() {
        assert_eq!(Role::parse(""), None);
        assert_eq!(Role::parse("   "), None);
        assert_eq!(Role::parse("superuser"), None);
        assert_eq!(Role::parse("viewer2"), None);
        assert_eq!(Role::parse("operator admin"), None);
        assert_eq!(Role::parse("admins"), None);
    }

    #[test]
    fn role_as_str_roundtrips_through_parse() {
        for role in [Role::Viewer, Role::Operator, Role::Approver, Role::Admin] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
            assert_eq!(role.to_string(), role.as_str());
        }
        assert_eq!(Role::Admin.as_str(), "admin");
    }

    // ── Password hashing ──────────────────────────────────────────

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("correct horse battery staple").expect("hashing works");
        assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify_password(&hash, "correct horse battery staple"));
    }

    #[test]
    fn verify_rejects_wrong_password() {
        let hash = hash_password("hunter2").expect("hashing works");
        assert!(!verify_password(&hash, "hunter3"));
        assert!(!verify_password(&hash, ""));
    }

    #[test]
    fn verify_rejects_malformed_hash_without_panicking() {
        assert!(!verify_password("not-a-phc-string", "pw"));
        assert!(!verify_password("", "pw"));
        assert!(!verify_password(
            "$argon2id$v=19$m=19456,t=2,p=1$garbage",
            "pw"
        ));
        // A bcrypt hash is a well-formed PHC string argon2 cannot verify.
        assert!(!verify_password(
            "$2y$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy",
            "pw"
        ));
    }

    #[test]
    fn hashes_use_fresh_salts() {
        let a = hash_password("same-password").expect("hashing works");
        let b = hash_password("same-password").expect("hashing works");
        assert_ne!(a, b);
    }

    // ── Dummy hash (timing equalization) ─────────────────────────

    #[test]
    fn dummy_hash_is_valid_phc_but_matches_no_password() {
        let hash = dummy_password_hash();
        assert!(hash.starts_with("$argon2id$"));
        assert!(
            PasswordHash::new(hash).is_ok(),
            "dummy hash must parse as PHC"
        );
        assert!(!verify_password(hash, "any-password"));
        assert!(!verify_password(hash, ""));
        assert!(!verify_password(hash, "dummy"));
        // Cached: the same string is returned on every call.
        assert!(std::ptr::eq(dummy_password_hash(), hash));
    }

    // ── Identifier / credential generation ────────────────────────

    #[test]
    fn session_id_is_64_lowercase_hex_chars() {
        let id = new_session_id();
        assert_eq!(id.len(), 64);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(new_session_id(), id);
    }

    #[test]
    fn token_has_kro_prefix_and_64_hex_chars() {
        let token = new_token();
        assert_eq!(token.len(), 68);
        assert!(token.starts_with("kro_"));
        assert!(token[4..]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(new_token(), token);
    }

    #[test]
    fn password_is_24_alphanumeric_chars() {
        for _ in 0..64 {
            let pw = new_password();
            assert_eq!(pw.len(), 24);
            assert!(pw
                .chars()
                .all(|c| c.is_ascii_alphanumeric() && c.is_ascii()));
        }
    }

    // ── sha256_hex ────────────────────────────────────────────────

    #[test]
    fn sha256_hex_matches_known_vector() {
        // NIST-known digest of "abc".
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ── constant_time_eq ──────────────────────────────────────────

    #[test]
    fn constant_time_eq_compares_values() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(constant_time_eq("", ""));
        assert!(!constant_time_eq("secret", "secreu"));
        // Length-mismatched inputs are unequal but must not panic.
        assert!(!constant_time_eq("secret", "secre"));
        assert!(!constant_time_eq("secret", "secretxyz"));
        assert!(!constant_time_eq("", "a"));
    }

    // ── host_is_loopback ──────────────────────────────────────────

    #[test]
    fn loopback_spellings_are_recognised() {
        assert!(host_is_loopback("127.0.0.1"));
        assert!(host_is_loopback("127.0.0.2"));
        assert!(host_is_loopback("localhost"));
        assert!(host_is_loopback("  localhost  "));
        assert!(host_is_loopback("::1"));
        assert!(host_is_loopback("[::1]"));
        assert!(host_is_loopback("[127.0.0.1]"));
    }

    #[test]
    fn network_exposed_hosts_are_not_loopback() {
        assert!(!host_is_loopback("0.0.0.0"));
        assert!(!host_is_loopback("::"));
        assert!(!host_is_loopback("[::]"));
        assert!(!host_is_loopback("192.168.1.10"));
        assert!(!host_is_loopback("10.0.0.5"));
        assert!(!host_is_loopback("example.com"));
        assert!(!host_is_loopback(""));
    }
}

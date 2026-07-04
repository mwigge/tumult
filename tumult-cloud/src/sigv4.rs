//! AWS Signature Version 4 request signing.
//!
//! A minimal, dependency-light implementation of the [SigV4 signing process]
//! using pure-Rust `sha2` + `hmac`. It produces the `Authorization`,
//! `X-Amz-Date`, and (for temporary credentials) `X-Amz-Security-Token`
//! headers to attach to an otherwise plain `reqwest` request.
//!
//! Correctness is pinned by [`tests`] against the canonical `get-vanilla`
//! vector from the AWS `SigV4` test suite.
//!
//! [SigV4 signing process]: https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv4-signing-elements.html

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::creds::AwsCredentials;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256. The key length is unconstrained, so construction never fails.
fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(key).unwrap_or_else(|_| unreachable!("HMAC key any length"));
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Lowercase hex SHA-256 of `data`.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// RFC 3986 percent-encode. When `encode_slash` is false the `/` byte is
/// passed through unescaped (used for canonical URI path segments).
fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for &byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            b'/' if !encode_slash => out.push('/'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(char::from(HEX[(byte >> 4) as usize]));
                out.push(char::from(HEX[(byte & 0x0f) as usize]));
            }
        }
    }
    out
}

/// RFC 3986 percent-encode `input`, escaping every reserved byte (including
/// `/`). Used to encode EC2 Query-protocol parameter values.
#[must_use]
pub fn encode_query_value(input: &str) -> String {
    uri_encode(input, true)
}

/// The parts of an HTTP request that participate in a `SigV4` signature.
pub struct SignRequest<'a> {
    /// HTTP method, uppercase (`GET`, `POST`, `DELETE`).
    pub method: &'a str,
    /// Host authority as it will appear in the `Host` header (`host` or
    /// `host:port`).
    pub host: &'a str,
    /// Absolute request path, beginning with `/`.
    pub path: &'a str,
    /// Canonical (already sorted / encoded) query string, or empty.
    pub query: &'a str,
    /// Raw request body bytes (empty slice for none).
    pub body: &'a [u8],
    /// Signing service name (`fis`, `ec2`).
    pub service: &'a str,
    /// AWS region (`us-east-1`).
    pub region: &'a str,
    /// Additional headers to include in the request *and* the signature
    /// (e.g. `content-type`). Names may be any case; values are trimmed.
    pub extra_headers: &'a [(String, String)],
}

/// Sign `req` and return the headers to attach: `Authorization`,
/// `X-Amz-Date`, and `X-Amz-Security-Token` when the credentials carry a
/// session token.
#[must_use]
pub fn sign(
    req: &SignRequest,
    creds: &AwsCredentials,
    now: DateTime<Utc>,
) -> Vec<(String, String)> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();

    // Build the full set of headers that are signed: host + x-amz-date, any
    // caller extras, and the session token when present. Lowercased, sorted.
    let mut headers: Vec<(String, String)> = vec![
        ("host".to_string(), req.host.to_string()),
        ("x-amz-date".to_string(), amz_date.clone()),
    ];
    for (name, value) in req.extra_headers {
        headers.push((name.to_lowercase(), value.trim().to_string()));
    }
    if let Some(token) = &creds.session_token {
        headers.push(("x-amz-security-token".to_string(), token.clone()));
    }
    headers.sort_by(|a, b| a.0.cmp(&b.0));

    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let mut canonical_headers = String::new();
    for (name, value) in &headers {
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value);
        canonical_headers.push('\n');
    }

    let payload_hash = sha256_hex(req.body);
    let canonical_uri = uri_encode(req.path, false);
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method, canonical_uri, req.query, canonical_headers, signed_headers, payload_hash
    );

    let scope = format!("{date_stamp}/{}/{}/aws4_request", req.region, req.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac(
        format!("AWS4{}", creds.secret_access_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac(&k_date, req.region.as_bytes());
    let k_service = hmac(&k_region, req.service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        creds.access_key_id
    );

    let mut out = vec![
        ("Authorization".to_string(), authorization),
        ("X-Amz-Date".to_string(), amz_date),
    ];
    if let Some(token) = &creds.session_token {
        out.push(("X-Amz-Security-Token".to_string(), token.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn get_vanilla_matches_aws_test_suite_vector() {
        // Canonical `get-vanilla` case from the AWS SigV4 test suite.
        let creds = AwsCredentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        };
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let req = SignRequest {
            method: "GET",
            host: "example.amazonaws.com",
            path: "/",
            query: "",
            body: b"",
            service: "service",
            region: "us-east-1",
            extra_headers: &[],
        };
        let headers = sign(&req, &creds, now);
        let auth = &headers[0].1;
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
        assert_eq!(
            headers[1],
            ("X-Amz-Date".to_string(), "20150830T123600Z".to_string())
        );
    }

    #[test]
    fn session_token_adds_security_token_header() {
        let creds = AwsCredentials {
            access_key_id: "AKIA".to_string(),
            secret_access_key: "secret".to_string(),
            session_token: Some("session".to_string()),
        };
        let now = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let req = SignRequest {
            method: "POST",
            host: "fis.us-east-1.amazonaws.com",
            path: "/experiments",
            query: "",
            body: b"{}",
            service: "fis",
            region: "us-east-1",
            extra_headers: &[("content-type".to_string(), "application/json".to_string())],
        };
        let headers = sign(&req, &creds, now);
        assert!(headers
            .iter()
            .any(|(name, value)| name == "X-Amz-Security-Token" && value == "session"));
        // Signed-header list must include the extra and the token, sorted.
        assert!(headers[0]
            .1
            .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token"));
    }

    #[test]
    fn uri_encode_escapes_reserved_bytes() {
        assert_eq!(uri_encode("a b/c", false), "a%20b/c");
        assert_eq!(uri_encode("a b/c", true), "a%20b%2Fc");
        assert_eq!(uri_encode("i-0abc.DEF_9~", true), "i-0abc.DEF_9~");
    }
}

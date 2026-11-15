//! The `Content-Encoding` of an OTLP/HTTP body.
//!
//! The Collector's `otlphttp` exporter sends gzip unless it is told
//! otherwise, so gzip is read, next to a body sent as it is. What a
//! compressed body expands to is bounded as a message is on gRPC, so a
//! small body cannot make the daemon hold a large one. Any other encoding
//! is refused by name: a body that cannot be read is never accepted.

use std::io::Read;

use axum::body::Bytes;
use axum::http::{header, HeaderMap};
use flate2::read::MultiGzDecoder;
use prost::Message;

/// The most a compressed body may expand to: what tonic takes as one
/// message on the gRPC services (4 MiB).
pub(super) const MAX_DECODED_BYTES: usize = 4 * 1024 * 1024;

/// The longest name of an encoding repeated back to the sender.
const NAMED_LEN: usize = 32;

/// Why a body was not read.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The body is in an encoding the daemon does not read.
    Unsupported(String),
    /// The body expands to more than [`MAX_DECODED_BYTES`].
    TooLarge,
    /// The body is not what its encoding or its content type says.
    Malformed(String),
    /// The body could not be expanded for a reason that is the daemon's.
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Encoding {
    Identity,
    Gzip,
}

/// The encoding the headers name. None, an empty one and `identity` are a
/// body as it is; the name is read without regard to case. Several
/// encodings, on one line or on several, are not read.
fn encoding(headers: &HeaderMap) -> Result<Encoding, Refusal> {
    let mut named = headers.get_all(header::CONTENT_ENCODING).iter();
    let Some(first) = named.next() else {
        return Ok(Encoding::Identity);
    };
    if named.next().is_some() {
        return Err(Refusal::Unsupported("several encodings".into()));
    }
    let name = String::from_utf8_lossy(first.as_bytes());
    match name.trim().to_ascii_lowercase().as_str() {
        "" | "identity" => Ok(Encoding::Identity),
        "gzip" => Ok(Encoding::Gzip),
        other => Err(Refusal::Unsupported(
            other.chars().take(NAMED_LEN).collect(),
        )),
    }
}

/// Expand a gzip `body`, reading no more than one byte past `max`. A body
/// of several members (RFC 1952) is what they expand to one after the
/// other, bounded together; what follows a member is a member or an error,
/// so no part of a body goes unread.
fn inflate(body: &[u8], max: usize) -> Result<Vec<u8>, Refusal> {
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let mut plain = Vec::new();
    MultiGzDecoder::new(body)
        .take(limit)
        .read_to_end(&mut plain)
        .map_err(|error| Refusal::Malformed(format!("failed to decode the gzip body: {error}")))?;
    if plain.len() > max {
        return Err(Refusal::TooLarge);
    }
    Ok(plain)
}

/// The body as the sender wrote it before encoding it.
async fn plain(headers: &HeaderMap, body: Bytes) -> Result<Bytes, Refusal> {
    match encoding(headers)? {
        Encoding::Identity => Ok(body),
        Encoding::Gzip => {
            let sent = body.len();
            // Expanding is work for a core, not for the reactor.
            let plain = tokio::task::spawn_blocking(move || inflate(&body, MAX_DECODED_BYTES))
                .await
                .map_err(|error| Refusal::Failed(error.to_string()))??;
            tracing::debug!(
                name: "ingest.http.gzip_body",
                {
                    http.request.body.size = sent,
                    http.request.body.decoded_size = plain.len(),
                },
                "a gzip body was expanded"
            );
            Ok(Bytes::from(plain))
        }
    }
}

/// The export request a body carries, whatever its encoding.
pub(super) async fn message<T: Message + Default>(
    headers: &HeaderMap,
    body: Bytes,
) -> Result<T, Refusal> {
    let plain = plain(headers, body).await?;
    T::decode(plain)
        .map_err(|error| Refusal::Malformed(crate::error::IngestError::Decode(error).to_string()))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use axum::http::HeaderValue;
    use flate2::write::GzEncoder;
    use flate2::Compression;

    use super::*;

    fn gzip(plain: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plain).unwrap();
        encoder.finish().unwrap()
    }

    fn named(values: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::CONTENT_ENCODING, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn a_body_of_exactly_the_bound_is_read_and_one_byte_more_is_not() {
        let at_the_bound = vec![b'x'; 1024];
        assert_eq!(inflate(&gzip(&at_the_bound), 1024), Ok(at_the_bound));
        let one_more = vec![b'x'; 1025];
        assert_eq!(inflate(&gzip(&one_more), 1024), Err(Refusal::TooLarge));
    }

    #[test]
    fn the_members_of_a_gzip_body_are_read_as_one() {
        let body = [gzip(b"one, "), gzip(b"two")].concat();
        assert_eq!(inflate(&body, 1024), Ok(b"one, two".to_vec()));
    }

    #[test]
    fn the_members_of_a_gzip_body_are_bounded_together() {
        let body = [gzip(&[b'x'; 600]), gzip(&[b'x'; 600])].concat();
        assert_eq!(inflate(&body, 1024), Err(Refusal::TooLarge));
    }

    #[test]
    fn what_follows_a_member_and_is_not_one_is_malformed() {
        let mut body = gzip(b"one");
        body.extend_from_slice(b"not a gzip member");
        assert!(matches!(inflate(&body, 1024), Err(Refusal::Malformed(_))));
    }

    #[test]
    fn the_encoding_is_read_from_one_header_without_regard_to_case() {
        assert_eq!(encoding(&named(&[])), Ok(Encoding::Identity));
        assert_eq!(encoding(&named(&[""])), Ok(Encoding::Identity));
        assert_eq!(encoding(&named(&[" Identity "])), Ok(Encoding::Identity));
        assert_eq!(encoding(&named(&["GZIP"])), Ok(Encoding::Gzip));
        assert_eq!(
            encoding(&named(&["zstd"])),
            Err(Refusal::Unsupported("zstd".into()))
        );
        assert_eq!(
            encoding(&named(&["gzip", "gzip"])),
            Err(Refusal::Unsupported("several encodings".into()))
        );
    }

    #[test]
    fn a_long_name_is_cut_before_it_is_repeated() {
        let long: &'static str = "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz";
        let Err(Refusal::Unsupported(name)) = encoding(&named(&[long])) else {
            panic!("an encoding nobody knows was read");
        };
        assert_eq!(name, &long[..NAMED_LEN]);
    }
}

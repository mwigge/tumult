//! Making values safe to log.
//!
//! A URL is shown clean or not at all. [`sanitize_url`] and
//! [`server_address`] accept one reading only, the strict one, and answer
//! with [`REDACTED_URL`] for anything else: a value that is cut where it
//! merely looks like a URL ends shows whatever stood before that point, and
//! that can be the password.

/// What stands in for a URL, or a host, that cannot be shown to be free of
/// credentials. No part of the input is in it.
pub const REDACTED_URL: &str = "[redacted-url]";

/// What stands in for a secret value inside a longer text.
const REDACTED: &str = "[redacted]";

/// A query, fragment or path parameter value shorter than this is not
/// treated as a secret by [`redact_url`]: it cannot be one, and replacing
/// every `1` in an error text would leave nothing to read.
const MIN_PARAMETER_SECRET_BYTES: usize = 3;

/// A user name shorter than this is redacted only where it stands as a
/// word of its own: `ab` in `user ab` and `ab@host`, not in `tab`.
const MIN_USER_NAME_BYTES: usize = 3;

/// The longest secret that is searched for. The search costs text length
/// times secret length, so a text is not searched for a longer one: it is
/// replaced as a whole.
const MAX_SECRET_BYTES: usize = 4096;

/// The schemes a URL is read with. Anything else before `://` may be a
/// user name (`alice://password@host`), so such a value is not shown.
const KNOWN_SCHEMES: [&str; 30] = [
    "amqp",
    "amqps",
    "clickhouse",
    "file",
    "ftp",
    "ftps",
    "grpc",
    "grpcs",
    "h2c",
    "http",
    "https",
    "kafka",
    "mongodb",
    "mqtt",
    "mysql",
    "nats",
    "otlp",
    "postgres",
    "postgresql",
    "redis",
    "rediss",
    "s3",
    "sftp",
    "ssh",
    "tcp",
    "tls",
    "tumult",
    "udp",
    "ws",
    "wss",
];

/// The URL without the parts that can carry credentials — the userinfo
/// (`user:password@`), the query string, the fragment and path parameters
/// (`;key=value`) — so the result is safe to log as `url.full`.
///
/// Accepts `[scheme://][userinfo@]host[:port][/path]` and nothing else,
/// with a scheme this module knows (`http`, `https`, `grpc`,
/// `clickhouse`, ...). The answer is [`REDACTED_URL`] when the value is
/// not of that form, and when an `@` stands after the first `/`, `?` or
/// `#`: that is either a path with an `@` in it or a password with an
/// unescaped delimiter, and nothing in the value says which.
#[must_use]
pub fn sanitize_url(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    CleanUrl::parse(raw).map_or_else(|| REDACTED_URL.to_string(), |clean| clean.spell())
}

/// The host and port a URL points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    /// The host name or address, without brackets around an IPv6 address;
    /// [`REDACTED_URL`] when the URL could not be read. Safe to record as
    /// `server.address`.
    pub host: String,
    /// The port, when the URL names one. Recorded as `server.port`.
    pub port: Option<u16>,
}

/// The host and port of `url`, read as strictly as [`sanitize_url`] reads
/// it: the host is [`REDACTED_URL`] when the URL is not provably clean.
#[must_use]
pub fn server_address(url: &str) -> ServerAddress {
    let Some(clean) = CleanUrl::parse(url) else {
        return ServerAddress {
            host: REDACTED_URL.to_string(),
            port: None,
        };
    };
    let host = clean
        .host
        .strip_prefix('[')
        .and_then(|address| address.strip_suffix(']'))
        .unwrap_or(clean.host);
    ServerAddress {
        host: host.to_string(),
        port: clean.port,
    }
}

/// Replaces the secrets of `url` wherever they stand in `text`.
///
/// Error messages for a bad endpoint usually quote the endpoint, and not as
/// it was configured: with another case, port, path, parameter order or
/// percent-encoding, or only a part of it. So this does not look for the
/// URL. It takes the credentials out of `url` — the user name, the password
/// (the whole userinfo when it has no `:`), and every query, fragment and
/// path parameter value of at least three bytes — and replaces each
/// occurrence of one with `[redacted]`, whether the text spells it raw,
/// percent-encoded or a mix of both. A userinfo that is redacted in full
/// (`[redacted]:[redacted]@`) is then removed.
///
/// A user name under three bytes is replaced only where it is a word of
/// its own. When a credential is longer than 4096 bytes the text is not
/// searched: the answer is `[redacted]` alone.
#[must_use]
pub fn redact_url(text: &str, url: &str) -> String {
    let mut secrets = Secrets::default();
    secrets.add_url(url);
    secrets.redact(text)
}

/// [`redact_url`], with a user name and a password that are configured
/// beside the URL redacted in the same way. Empty ones are ignored.
#[must_use]
pub fn redact_credentials(text: &str, url: &str, user: &str, password: &str) -> String {
    let mut secrets = Secrets::default();
    secrets.add_url(url);
    secrets.add_user(user);
    secrets.add_value(password, 1);
    secrets.redact(text)
}

/// A URL that was read in full, with nothing left over.
struct CleanUrl<'a> {
    scheme: Option<&'a str>,
    /// An IPv6 address keeps its brackets.
    host: &'a str,
    port: Option<u16>,
    /// The path without parameters; empty or starting with `/`.
    path: String,
}

impl<'a> CleanUrl<'a> {
    /// `None` unless `raw` is `[scheme://][userinfo@]host[:port][/path]`
    /// with an optional query and fragment, and no `@` past the authority.
    fn parse(raw: &'a str) -> Option<Self> {
        let (scheme, rest) = split_scheme(raw)?;
        let (authority, beyond) = rest.split_at(end_of_authority(rest));
        if beyond.contains('@') {
            return None;
        }
        let host_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_userinfo, host_port)| host_port);
        let (host, port) = split_host_port(host_port)?;
        let path = &beyond[..beyond.find(['?', '#']).unwrap_or(beyond.len())];
        Some(Self {
            scheme,
            host,
            port,
            path: without_parameters(path),
        })
    }

    fn spell(&self) -> String {
        let scheme = self
            .scheme
            .map(|scheme| format!("{scheme}://"))
            .unwrap_or_default();
        let port = self.port.map(|port| format!(":{port}")).unwrap_or_default();
        format!("{scheme}{}{port}{}", self.host, self.path)
    }
}

/// Where the authority of `rest` (a URL from just after its scheme) ends:
/// at the first `/`, `?` or `#`.
fn end_of_authority(rest: &str) -> usize {
    rest.find(['/', '?', '#']).unwrap_or(rest.len())
}

/// Splits `scheme://` off the front. A value without `://` has no scheme;
/// one with something other than a known scheme before it is not shown.
fn split_scheme(raw: &str) -> Option<(Option<&str>, &str)> {
    let Some((scheme, rest)) = raw.split_once("://") else {
        return Some((None, raw));
    };
    is_known_scheme(scheme).then_some((Some(scheme), rest))
}

fn is_known_scheme(candidate: &str) -> bool {
    KNOWN_SCHEMES
        .iter()
        .any(|scheme| scheme.eq_ignore_ascii_case(candidate))
}

/// Splits `host[:port]`; `None` unless the host is a name or address and
/// the port, when there is one, a port number.
fn split_host_port(host_port: &str) -> Option<(&str, Option<u16>)> {
    let (host, after) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let (address, after) = bracketed.split_once(']')?;
        is_ipv6_address(address).then_some(())?;
        (&host_port[..address.len() + 2], after)
    } else {
        let colon = host_port.find(':').unwrap_or(host_port.len());
        let (name, after) = host_port.split_at(colon);
        is_host_name(name).then_some(())?;
        (name, after)
    };
    if after.is_empty() {
        return Some((host, None));
    }
    let port = after.strip_prefix(':')?;
    let all_digits = !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit());
    all_digits.then_some(())?;
    Some((host, Some(port.parse().ok()?)))
}

fn is_host_name(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn is_ipv6_address(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
}

/// `path` with the `;parameters` of every segment removed.
fn without_parameters(path: &str) -> String {
    let segments: Vec<&str> = path
        .split('/')
        .map(|segment| segment.split(';').next().unwrap_or(segment))
        .collect();
    segments.join("/")
}

/// One value that must not be quoted.
struct Secret {
    /// Percent-decoded.
    bytes: Vec<u8>,
    /// Replaced only where no letter or digit stands next to it.
    whole_word: bool,
}

impl Secret {
    /// Where this secret ends when `rest` starts with it. `previous` is
    /// the character before `rest`.
    fn end_at_start(&self, rest: &str, previous: Option<char>) -> Option<usize> {
        let end = spelled_at_start(rest, &self.bytes)?;
        if !rest.is_char_boundary(end) {
            return None;
        }
        if !self.whole_word {
            return Some(end);
        }
        let next = rest[end..].chars().next();
        let inside_a_word = [previous, next]
            .into_iter()
            .flatten()
            .any(char::is_alphanumeric);
        (!inside_a_word).then_some(end)
    }
}

/// What must not be quoted from a URL and the credentials beside it.
#[derive(Default)]
struct Secrets {
    values: Vec<Secret>,
    /// A value was longer than [`MAX_SECRET_BYTES`].
    too_long: bool,
}

impl Secrets {
    /// Reads `url` generously: every reading of it that yields a userinfo
    /// counts, since a library may have quoted it under any of them.
    fn add_url(&mut self, url: &str) {
        let (scheme, rest) = url
            .split_once("://")
            .map_or((None, url), |(scheme, rest)| (Some(scheme), rest));
        if let Some(scheme) = scheme.filter(|scheme| !is_known_scheme(scheme)) {
            // `alice://password@host`.
            self.add_user(scheme);
        }
        let strict = rest[..end_of_authority(rest)].rfind('@');
        let generous = rest.rfind('@');
        for at in [strict, generous].into_iter().flatten() {
            self.add_userinfo(&rest[..at]);
        }
        for value in parameter_values(rest) {
            self.add_value(value, MIN_PARAMETER_SECRET_BYTES);
        }
    }

    fn add_userinfo(&mut self, userinfo: &str) {
        let Some((user, password)) = userinfo.split_once(':') else {
            // No password: the userinfo is a token.
            self.add_value(userinfo, 1);
            return;
        };
        self.add_value(password, 1);
        self.add_user(user);
    }

    fn add_user(&mut self, user: &str) {
        let bytes = percent_decode(user.as_bytes());
        let whole_word = bytes.len() < MIN_USER_NAME_BYTES;
        self.add(bytes, 1, whole_word);
    }

    fn add_value(&mut self, value: &str, min_bytes: usize) {
        self.add(percent_decode(value.as_bytes()), min_bytes, false);
    }

    fn add(&mut self, bytes: Vec<u8>, min_bytes: usize, whole_word: bool) {
        if bytes.len() > MAX_SECRET_BYTES {
            self.too_long = true;
            return;
        }
        if bytes.len() >= min_bytes {
            self.values.push(Secret { bytes, whole_word });
        }
    }

    /// `text` without the secrets. Fails closed: a secret too long to
    /// search for leaves nothing of the text.
    fn redact(mut self, text: &str) -> String {
        if self.too_long {
            return REDACTED.to_string();
        }
        // Longest first, so a secret is not cut short by one it contains.
        self.values
            .sort_by_key(|secret| (std::cmp::Reverse(secret.bytes.len()), secret.whole_word));
        self.values
            .dedup_by(|later, kept| later.bytes == kept.bytes);
        replace_values(text, &self.values)
            .replace(&format!("{REDACTED}:{REDACTED}@"), "")
            .replace(&format!("{REDACTED}:@"), "")
    }
}

/// The values of the query, fragment and path parameters of `rest`, a URL
/// from just after its scheme. A parameter without `=` is its own value.
fn parameter_values(rest: &str) -> impl Iterator<Item = &str> {
    let (head, tail) = rest.split_at(rest.find(['?', '#']).unwrap_or(rest.len()));
    let in_path = head
        .split('/')
        .flat_map(|segment| segment.split(';').skip(1));
    let in_tail = tail.split(['?', '#', '&', ';']);
    in_path
        .chain(in_tail)
        .filter(|parameter| !parameter.is_empty())
        .map(|parameter| {
            parameter
                .split_once('=')
                .map_or(parameter, |(_key, value)| value)
        })
}

/// `text` with every occurrence of one of `values` replaced by
/// [`REDACTED`]. `values` are tried in order.
fn replace_values(text: &str, values: &[Secret]) -> String {
    let mut redacted = String::with_capacity(text.len());
    let mut rest = text;
    let mut previous = None;
    while let Some(character) = rest.chars().next() {
        let matched = values
            .iter()
            .find_map(|secret| secret.end_at_start(rest, previous));
        let end = matched.unwrap_or(character.len_utf8());
        if matched.is_some() {
            redacted.push_str(REDACTED);
        } else {
            redacted.push(character);
        }
        previous = rest[..end].chars().next_back();
        rest = &rest[end..];
    }
    redacted
}

/// Where `value` ends when `text` starts with it, each byte spelled either
/// as itself or percent-encoded.
fn spelled_at_start(text: &str, value: &[u8]) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    for &expected in value {
        at += spelling_len(&bytes[at..], expected)?;
    }
    (at > 0).then_some(at)
}

/// How many bytes at the start of `bytes` spell `expected`: three for
/// `%XX`, one for the byte itself.
fn spelling_len(bytes: &[u8], expected: u8) -> Option<usize> {
    if bytes.get(..3).and_then(escaped_byte) == Some(expected) {
        return Some(3);
    }
    (bytes.first() == Some(&expected)).then_some(1)
}

/// The byte a `%XX` escape stands for.
fn escaped_byte(escape: &[u8]) -> Option<u8> {
    let [b'%', high, low] = escape else {
        return None;
    };
    let high = char::from(*high).to_digit(16)?;
    let low = char::from(*low).to_digit(16)?;
    u8::try_from(high * 16 + low).ok()
}

/// `raw` with every valid `%XX` escape replaced by its byte.
fn percent_decode(raw: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut at = 0;
    while let Some(&byte) = raw.get(at) {
        if let Some(escaped) = raw.get(at..at + 3).and_then(escaped_byte) {
            decoded.push(escaped);
            at += 3;
            continue;
        }
        decoded.push(byte);
        at += 1;
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::{redact_credentials, redact_url, sanitize_url, server_address, ServerAddress};

    /// What stands in for a URL that cannot be shown to be clean.
    const PLACEHOLDER: &str = "[redacted-url]";

    /// Whether any three bytes in a row of `secret` occur in `shown`.
    fn leaks(shown: &str, secret: &str) -> bool {
        secret
            .as_bytes()
            .windows(3)
            .any(|window| shown.as_bytes().windows(3).any(|seen| seen == window))
    }

    /// (input, sanitised URL, host, port, the secrets in the input)
    type Case = (
        &'static str,
        &'static str,
        &'static str,
        Option<u16>,
        &'static [&'static str],
    );

    const CASES: &[Case] = &[
        // Userinfo that holds a character which ends an authority: nothing
        // of the input can be trusted to be the host.
        (
            "http://u:hun#ter2@h:8123/db",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["hun#ter2"],
        ),
        (
            "http://u:hun?ter2@h/db",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["hun?ter2"],
        ),
        (
            "http://u:123/456@h",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["123/456"],
        ),
        (
            "http://token/en77@h",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["token/en77"],
        ),
        (
            "http://u:pa/ss@host",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["pa/ss"],
        ),
        ("http:/u:pw9@h", PLACEHOLDER, PLACEHOLDER, None, &["pw9"]),
        // An `@` past the authority may be a path, or the end of a
        // password: not provably clean.
        (
            "https://example.com/users/@alice",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &[],
        ),
        (
            "http://host/x?token=abc@def",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["abc@def"],
        ),
        // Not a URL.
        (
            "http://host:notaport/x",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &[],
        ),
        ("http://host:/x", PLACEHOLDER, PLACEHOLDER, None, &[]),
        ("http://host:99999/x", PLACEHOLDER, PLACEHOLDER, None, &[]),
        ("http://ho st/x", PLACEHOLDER, PLACEHOLDER, None, &[]),
        ("ht!tp://host/x", PLACEHOLDER, PLACEHOLDER, None, &[]),
        ("http://[::1/x", PLACEHOLDER, PLACEHOLDER, None, &[]),
        (
            "http://u:hunter2@/x",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["hunter2"],
        ),
        // What stands before `://` is a user name unless it is a scheme
        // this code knows.
        (
            "alice://hunter2@ch:9000",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["alice", "hunter2"],
        ),
        (
            "alice://ch:9000",
            PLACEHOLDER,
            PLACEHOLDER,
            None,
            &["alice"],
        ),
        // Clean readings.
        (
            "clickhouse://bob:hunter2@ch:9000/db",
            "clickhouse://ch:9000/db",
            "ch",
            Some(9000),
            &["bob", "hunter2"],
        ),
        (
            "GRPCS://collector:4317",
            "GRPCS://collector:4317",
            "collector",
            Some(4317),
            &[],
        ),
        (
            "http://h:8123/db?password=hunter2",
            "http://h:8123/db",
            "h",
            Some(8123),
            &["hunter2"],
        ),
        (
            "http://h/db;password=hunter2",
            "http://h/db",
            "h",
            None,
            &["hunter2"],
        ),
        (
            "http://h/a;p=hunter2/b;q=s3cret?x=y",
            "http://h/a/b",
            "h",
            None,
            &["hunter2", "s3cret"],
        ),
        (
            "http://u:hun@ter2@host/x",
            "http://host/x",
            "host",
            None,
            &["hun@ter2"],
        ),
        (
            "http://token123@host",
            "http://host",
            "host",
            None,
            &["token123"],
        ),
        (
            "http://u:hunter2@[::1]:8123/db",
            "http://[::1]:8123/db",
            "::1",
            Some(8123),
            &["hunter2"],
        ),
        (
            "http://[2001:db8::7]/x",
            "http://[2001:db8::7]/x",
            "2001:db8::7",
            None,
            &[],
        ),
        (
            "u:hunter2@host:9000",
            "host:9000",
            "host",
            Some(9000),
            &["hunter2"],
        ),
        (
            "localhost:9000",
            "localhost:9000",
            "localhost",
            Some(9000),
            &[],
        ),
        (
            "collector:4317/v1/logs",
            "collector:4317/v1/logs",
            "collector",
            Some(4317),
            &[],
        ),
        (
            "HTTP://u:hunter2@Host/x",
            "HTTP://Host/x",
            "Host",
            None,
            &["hunter2"],
        ),
        (
            "http://u:hun%2Fter2@host",
            "http://host",
            "host",
            None,
            &["hun%2Fter2"],
        ),
        (
            "https://collector.example/v1#token=abc123",
            "https://collector.example/v1",
            "collector.example",
            None,
            &["abc123"],
        ),
        (
            "http://u:p@[::1]:4318/hook/T000/B000?sig=xyz",
            "http://[::1]:4318/hook/T000/B000",
            "::1",
            Some(4318),
            &[],
        ),
        ("http://@host", "http://host", "host", None, &[]),
        (
            "https://clickhouse.internal",
            "https://clickhouse.internal",
            "clickhouse.internal",
            None,
            &[],
        ),
        ("", "", PLACEHOLDER, None, &[]),
    ];

    #[test]
    fn a_url_is_shown_clean_or_not_at_all() {
        for &(raw, url, host, port, secrets) in CASES {
            let sanitized = sanitize_url(raw);
            let address = server_address(raw);
            assert_eq!(sanitized, url, "{raw}");
            assert_eq!(
                address,
                ServerAddress {
                    host: host.to_string(),
                    port
                },
                "{raw}"
            );
            for secret in secrets {
                assert!(!leaks(&sanitized, secret), "{raw}: {sanitized}");
                assert!(!leaks(&address.host, secret), "{raw}: {}", address.host);
            }
        }
    }

    /// (configured URL, error text, redacted text, the secrets configured)
    type Quote = (
        &'static str,
        &'static str,
        &'static str,
        &'static [&'static str],
    );

    const QUOTES: &[Quote] = &[
        (
            "http://alice:hunter2@host:4317/?token=abc123",
            "invalid URI http://alice:hunter2@host:4317/?token=abc123. Reason: bad (http://alice:hunter2@host:4317/?token=abc123)",
            "invalid URI http://host:4317/?token=[redacted]. Reason: bad (http://host:4317/?token=[redacted])",
            &["hunter2", "abc123"],
        ),
        // Quoted in another spelling than configured.
        (
            "HTTP://alice:hunter2@Host:4317",
            "invalid URI http://alice:hunter2@host:4317/",
            "invalid URI http://host:4317/",
            &["hunter2"],
        ),
        (
            "https://alice:hunter2@host/v1",
            "no route to https://alice:hunter2@host:443/v1",
            "no route to https://host:443/v1",
            &["hunter2"],
        ),
        (
            "http://alice:hunter2@host:4317",
            "dns error for alice:hunter2@host.:4317",
            "dns error for host.:4317",
            &["hunter2"],
        ),
        // The userinfo in another percent-encoding than configured.
        (
            "http://u:hun%2Fter2@host",
            "bad url http://u:hun/ter2@host",
            "bad url http://host",
            &["hun/ter2", "hun%2Fter2"],
        ),
        (
            "http://u:hun/ter2@host",
            "bad url http://u:hun%2Fter2@host and http://u:hun%2fter2@host",
            "bad url http://host and http://host",
            &["hun/ter2", "hun%2Fter2", "hun%2fter2"],
        ),
        (
            "http://u:hun#ter2@h:8123/db",
            "cannot parse http://u:hun#ter2@h:8123/db (http://u:hun%23ter2@h:8123/db)",
            "cannot parse http://h:8123/db (http://h:8123/db)",
            &["hun#ter2", "hun%23ter2"],
        ),
        // A token in the query, quoted with another path, a trailing
        // slash, another parameter order, or on its own.
        (
            "http://host/a?token=t0psecret&mode=fast",
            "GET http://host/b/?mode=fast&token=t0psecret failed",
            "GET http://host/b/?mode=[redacted]&token=[redacted] failed",
            &["t0psecret"],
        ),
        (
            "http://host/a?token=t0psecret",
            "rejected: token=t0psecret",
            "rejected: token=[redacted]",
            &["t0psecret"],
        ),
        (
            "http://host/a?sig=a%2Bb%3Dc",
            "bad signature a+b=c (sig=a%2bb%3dc)",
            "bad signature [redacted] (sig=[redacted])",
            &["a+b=c", "a%2Bb%3Dc"],
        ),
        (
            "http://host/#access_token=abcdef",
            "fragment abcdef was sent",
            "fragment [redacted] was sent",
            &["abcdef"],
        ),
        (
            "http://host/db;password=hunter2",
            "the value hunter2 was refused",
            "the value [redacted] was refused",
            &["hunter2"],
        ),
        (
            "http://tok3n77@host",
            "dns error for tok3n77@host.",
            "dns error for [redacted]@host.",
            &["tok3n77"],
        ),
        // The user name is a credential too: with an empty password, in
        // another percent-encoding than configured, or without the
        // password.
        (
            "http://alice:@host",
            "bad url http://alice:@host",
            "bad url http://host",
            &["alice"],
        ),
        (
            "http://al%69ce:hunter2@host",
            "bad url http://alice:hunter2@host",
            "bad url http://host",
            &["alice", "hunter2"],
        ),
        (
            "http://alice:hunter2@host",
            "bad url http://al%69ce:hunter2@host",
            "bad url http://host",
            &["alice", "al%69ce", "hunter2"],
        ),
        (
            "http://alice:hunter2@host",
            "no user alice@host (alice is unknown)",
            "no user [redacted]@host ([redacted] is unknown)",
            &["alice", "hunter2"],
        ),
        // A user name under three bytes is redacted where it stands as a
        // word of its own, not inside other words.
        (
            "http://ab:hunter2@host",
            "tab ab@host (user ab, db=ab, %61b) cab",
            "tab [redacted]@host (user [redacted], db=[redacted], [redacted]) cab",
            &["hunter2"],
        ),
        (
            "http://ab:@host",
            "bad url http://ab:@host",
            "bad url http://host",
            &[],
        ),
        // A scheme that is not known may be a user name.
        (
            "alice://hunter2@ch:9000",
            "cannot reach alice://hunter2@ch:9000",
            "cannot reach [redacted]://[redacted]@ch:9000",
            &["alice", "hunter2"],
        ),
        // Nothing to redact.
        (
            "http://u:hunter2@host",
            "connection refused",
            "connection refused",
            &["hunter2"],
        ),
        (
            "http://host:4317",
            "cannot reach http://host:4317/ today",
            "cannot reach http://host:4317/ today",
            &[],
        ),
        ("", "anything", "anything", &[]),
        // A value this short is no secret, and replacing it would shred
        // the message.
        (
            "http://host/?debug=1&v=ab",
            "os error 111 (tab)",
            "os error 111 (tab)",
            &[],
        ),
    ];

    #[test]
    fn an_error_text_loses_the_secrets_of_the_url_in_any_spelling() {
        for &(configured, text, expected, secrets) in QUOTES {
            let redacted = redact_url(text, configured);
            assert_eq!(redacted, expected, "{configured}");
            for secret in secrets {
                assert!(!leaks(&redacted, secret), "{configured}: {redacted}");
            }
        }
    }

    #[test]
    fn a_secret_too_long_to_search_for_hides_the_whole_text() {
        let url = |bytes: usize| format!("http://host/?token={}", "x".repeat(bytes));

        assert_eq!(
            redact_url("connection refused", &url(4096)),
            "connection refused"
        );
        assert_eq!(redact_url("connection refused", &url(4097)), "[redacted]");
        let user = format!("http://{}:pw@host", "u".repeat(4097));
        assert_eq!(redact_url("connection refused", &user), "[redacted]");
    }

    #[test]
    fn credentials_configured_beside_the_url_are_redacted_like_its_own() {
        let url = "http://admin:url-pw@ch:8123";
        let text = "http://admin:url-pw@ch:8123: user svc-tumult (svc%2Dtumult) key pw-secret";
        assert_eq!(
            redact_credentials(text, url, "svc-tumult", "pw-secret"),
            "http://ch:8123: user [redacted] ([redacted]) key [redacted]"
        );
        // A short user name is a word of its own or nothing.
        assert_eq!(
            redact_credentials("tab ab", "http://ch:8123", "ab", ""),
            "tab [redacted]"
        );
        // Nothing configured: the text of `redact_url`.
        assert_eq!(redact_credentials(text, "", "", ""), text);
        let long = "p".repeat(4097);
        assert_eq!(redact_credentials(text, url, "svc", &long), "[redacted]");
    }

    #[test]
    fn a_redaction_never_splits_a_character() {
        let redacted = redact_url("lösenord pässword fel", "http://u:pässword@host");
        assert_eq!(redacted, "lösenord [redacted] fel");
    }
}

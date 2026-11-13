//! Making values safe to log.

/// Removes the parts of a URL that can carry credentials — the userinfo
/// (`user:password@`), the query string and the fragment — so the result is
/// safe to log as `url.full`.
///
/// Works on anything URL-shaped, with or without a scheme, and never fails:
/// a value that is not a URL is returned without its query and fragment.
///
/// A password with an unescaped `/` (`http://u:pa/ss@host`) is recognised
/// when what precedes the `/` cannot be a `host[:port]`; see
/// [`split_userinfo`] for the one form that stays ambiguous.
#[must_use]
pub fn sanitize_url(raw: &str) -> String {
    UrlParts::of(raw).sanitized()
}

/// Replaces every occurrence of `url` in `text` with its
/// [sanitised](sanitize_url) form.
///
/// Error messages for a bad endpoint usually quote the endpoint, and not
/// always as it was configured: libraries lower-case the scheme and host,
/// add or drop a trailing slash and the scheme's default port. Each of those
/// spellings is replaced, and whatever is left of the `user:password@` text
/// is removed wherever it stands, so the credentials stay out of
/// `error.message` even in a spelling nobody predicted.
#[must_use]
pub fn redact_url(text: &str, url: &str) -> String {
    let parts = UrlParts::of(url);
    let userinfo = parts.userinfo.filter(|userinfo| !userinfo.is_empty());
    if userinfo.is_none() && parts.tail.is_empty() {
        return text.to_string();
    }
    let mut redacted = text.to_string();
    for (quoted, clean) in parts.spellings() {
        redacted = redacted.replace(&quoted, &clean);
    }
    match userinfo {
        Some(userinfo) => redacted.replace(&format!("{userinfo}@"), ""),
        None => redacted,
    }
}

/// The host and port a URL points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    /// The host name or address, without brackets around an IPv6 address.
    /// Safe to record as `server.address`.
    pub host: String,
    /// The port, when the URL names one. Recorded as `server.port`.
    pub port: Option<u16>,
}

/// The host and port of `url`, with everything that can carry a credential
/// (userinfo, path, query, fragment) left out.
#[must_use]
pub fn server_address(url: &str) -> ServerAddress {
    let host_port = UrlParts::of(url).host_port;
    let port = port_of(host_port);
    let host = &host_port[..host_port.len() - port.map_or(0, |port| port.len() + 1)];
    let host = host
        .strip_prefix('[')
        .and_then(|address| address.strip_suffix(']'))
        .unwrap_or(host);
    ServerAddress {
        host: host.to_string(),
        port: port.and_then(|port| port.parse().ok()),
    }
}

/// A URL, split at the places a credential can hide.
struct UrlParts<'a> {
    scheme: Option<&'a str>,
    /// `user` or `user:password`, without the `@`.
    userinfo: Option<&'a str>,
    /// `host` or `host:port`; an IPv6 address keeps its brackets.
    host_port: &'a str,
    path: &'a str,
    /// The query and fragment, with their `?`/`#`; empty when absent.
    tail: &'a str,
}

impl<'a> UrlParts<'a> {
    /// Splits anything URL-shaped, with or without a scheme. Never fails:
    /// a value that is not a URL ends up as a host and a path.
    fn of(raw: &'a str) -> Self {
        let (head, tail) = raw.split_at(raw.find(['?', '#']).unwrap_or(raw.len()));
        let (scheme, rest) = match head.split_once("://") {
            Some((scheme, rest)) => (Some(scheme), rest),
            None => (None, head),
        };
        let (userinfo, located) = split_userinfo(rest);
        let (host_port, path) = located.split_at(located.find('/').unwrap_or(located.len()));
        Self {
            scheme,
            userinfo,
            host_port,
            path,
            tail,
        }
    }

    /// The URL without its userinfo, query and fragment.
    fn sanitized(&self) -> String {
        spell(self.scheme, None, self.host_port, self.path, "")
    }

    /// The spellings this URL may be quoted in, each paired with its
    /// sanitised form, longest first so that a spelling is never replaced
    /// through one of its own prefixes.
    fn spellings(&self) -> Vec<(String, String)> {
        let schemes = with_lowercase(self.scheme.map(str::to_string).into_iter().collect());
        let default_port = self.scheme.and_then(default_port);
        let host_ports = with_lowercase(host_port_spellings(self.host_port, default_port));
        let paths = match self.path {
            "" => vec!["", "/"],
            "/" => vec!["/", ""],
            path => vec![path],
        };
        let mut spellings = Vec::new();
        for scheme in schemes.iter().map(|scheme| Some(scheme.as_str())).chain(
            // A URL without a scheme has exactly one (absent) scheme.
            self.scheme.is_none().then_some(None),
        ) {
            for host_port in &host_ports {
                for path in &paths {
                    spellings.push((
                        spell(scheme, self.userinfo, host_port, path, self.tail),
                        spell(scheme, None, host_port, path, ""),
                    ));
                }
            }
        }
        spellings.sort_by_key(|(quoted, _)| std::cmp::Reverse(quoted.len()));
        spellings
    }
}

/// Puts a URL back together from its parts.
fn spell(
    scheme: Option<&str>,
    userinfo: Option<&str>,
    host_port: &str,
    path: &str,
    tail: &str,
) -> String {
    let scheme = scheme
        .map(|scheme| format!("{scheme}://"))
        .unwrap_or_default();
    let userinfo = userinfo
        .map(|userinfo| format!("{userinfo}@"))
        .unwrap_or_default();
    format!("{scheme}{userinfo}{host_port}{path}{tail}")
}

/// `values`, each followed by its lower-case form when that differs.
fn with_lowercase(values: Vec<String>) -> Vec<String> {
    let mut all = Vec::with_capacity(values.len() * 2);
    for value in values {
        let lower = value.to_ascii_lowercase();
        let differs = lower != value;
        all.push(value);
        if differs {
            all.push(lower);
        }
    }
    all
}

/// The port a scheme implies when the URL names none.
fn default_port(scheme: &str) -> Option<&'static str> {
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Some("80"),
        "https" => Some("443"),
        _ => None,
    }
}

/// `host_port` as configured, and with the scheme's default port dropped
/// (when it names it) or added (when it names none).
fn host_port_spellings(host_port: &str, default_port: Option<&str>) -> Vec<String> {
    let mut spellings = vec![host_port.to_string()];
    let Some(default_port) = default_port else {
        return spellings;
    };
    match port_of(host_port) {
        Some(port) if port == default_port => {
            let host = &host_port[..host_port.len() - port.len() - 1];
            spellings.push(host.to_string());
        }
        Some(_) => {}
        None => spellings.push(format!("{host_port}:{default_port}")),
    }
    spellings
}

/// Splits `user:password@` off the front of `rest`, a URL from just after
/// its scheme.
///
/// The authority ends at the first `/` and the userinfo at the last `@`
/// inside it. A password with an unescaped `/` breaks that rule:
/// `u:pa/ss@host` would read as host `u`, port `pa`, path `/ss@host`. No
/// host has such a port, so when what would be the host is not a plausible
/// `host[:port]`, the userinfo is taken to run up to the last `@` of the
/// whole value instead.
///
/// One form stays ambiguous and is read as the URL standard reads it:
/// `http://user/pass@host`, where the text before the `/` could be a host,
/// is host `user` with the path `/pass@host`.
fn split_userinfo(rest: &str) -> (Option<&str>, &str) {
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let at_in_authority = rest[..authority_end].rfind('@');
    let host_start = at_in_authority.map_or(0, |at| at + 1);
    let boundary = if is_host_port(&rest[host_start..authority_end]) {
        at_in_authority
    } else {
        rest.rfind('@')
    };
    match boundary {
        Some(at) => (Some(&rest[..at]), &rest[at + 1..]),
        None => (None, rest),
    }
}

/// What follows the host in `host_port`: empty, or `:port`. An IPv6
/// address is only a host inside brackets.
fn after_host(host_port: &str) -> &str {
    let bracketed = host_port
        .strip_prefix('[')
        .and_then(|address| address.split_once(']'));
    match bracketed {
        Some((_address, after)) => after,
        None => host_port.find(':').map_or("", |colon| &host_port[colon..]),
    }
}

/// The port named in `host_port`, if any, as written.
fn port_of(host_port: &str) -> Option<&str> {
    after_host(host_port).strip_prefix(':')
}

/// Whether `candidate` can be a `host` or `host:port`: nothing but an
/// optional all-digit port follows the host.
fn is_host_port(candidate: &str) -> bool {
    let after = after_host(candidate);
    match after.strip_prefix(':') {
        Some(port) => port.bytes().all(|byte| byte.is_ascii_digit()),
        None => after.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::{redact_url, sanitize_url, server_address, ServerAddress};

    #[test]
    fn server_address_is_the_host_and_port_only() {
        let cases = [
            (
                "http://admin:s3cret@clickhouse.internal:8123/db?password=x",
                "clickhouse.internal",
                Some(8123),
            ),
            ("https://clickhouse.internal", "clickhouse.internal", None),
            ("http://u:p@[::1]:8123", "::1", Some(8123)),
            ("http://[2001:db8::7]/x", "2001:db8::7", None),
            ("localhost:9000", "localhost", Some(9000)),
            ("http://u:pa/ss@host:8123", "host", Some(8123)),
            ("http://host:notaport/x", "host", None),
        ];
        for (url, host, port) in cases {
            let expected = ServerAddress {
                host: host.to_string(),
                port,
            };
            assert_eq!(server_address(url), expected, "{url}");
        }
    }

    #[test]
    fn redact_url_replaces_every_quoted_occurrence() {
        let url = "http://alice:hunter2@host:4317/?token=abc";
        let text = format!("invalid URI {url}. Reason: bad ({url})");
        assert_eq!(
            redact_url(&text, url),
            "invalid URI http://host:4317/. Reason: bad (http://host:4317/)"
        );
    }

    #[test]
    fn redact_url_leaves_unrelated_text_alone() {
        assert_eq!(
            redact_url("connection refused", "http://u:p@host"),
            "connection refused"
        );
        assert_eq!(redact_url("anything", ""), "anything");
    }

    #[test]
    fn plain_urls_are_unchanged() {
        assert_eq!(
            sanitize_url("http://localhost:4317"),
            "http://localhost:4317"
        );
        assert_eq!(
            sanitize_url("https://collector.example/v1/logs"),
            "https://collector.example/v1/logs"
        );
        assert_eq!(sanitize_url("collector:4317"), "collector:4317");
    }

    #[test]
    fn userinfo_is_removed() {
        assert_eq!(
            sanitize_url("https://alice:s3cret@collector.example:4317/v1"),
            "https://collector.example:4317/v1"
        );
        assert_eq!(sanitize_url("token@collector:4317"), "collector:4317");
    }

    #[test]
    fn query_and_fragment_are_removed() {
        assert_eq!(
            sanitize_url("https://collector.example/v1?api_key=abc123#frag"),
            "https://collector.example/v1"
        );
        assert_eq!(
            sanitize_url("https://collector.example/v1#token=abc123"),
            "https://collector.example/v1"
        );
    }

    #[test]
    fn everything_secret_is_removed_at_once() {
        let clean = sanitize_url("http://u:p@[::1]:4318/hook/T000/B000?sig=xyz");
        assert_eq!(clean, "http://[::1]:4318/hook/T000/B000");
    }

    #[test]
    fn an_at_sign_in_the_path_is_not_userinfo() {
        assert_eq!(
            sanitize_url("https://example.com/users/@alice"),
            "https://example.com/users/@alice"
        );
    }

    #[test]
    fn the_last_at_sign_in_the_authority_ends_the_userinfo() {
        assert_eq!(sanitize_url("https://a@b:c@host/x"), "https://host/x");
    }

    #[test]
    fn sanitize_url_table() {
        let cases = [
            // A password with a slash: the authority cannot be `u:pa`, so
            // the userinfo runs up to the last `@`.
            ("http://u:pa/ss@host", "http://host"),
            (
                "http://u:pa/ss@host:4317/v1/logs",
                "http://host:4317/v1/logs",
            ),
            ("https://u:p/a/b@c@host/x?k=v", "https://host/x"),
            ("u:pa/ss@host:4317", "host:4317"),
            // A well-formed authority keeps `@` in its path.
            (
                "https://example.com/users/@alice",
                "https://example.com/users/@alice",
            ),
            (
                "https://example.com:8443/a@b",
                "https://example.com:8443/a@b",
            ),
            ("http://[::1]:4318/a@b", "http://[::1]:4318/a@b"),
            ("http://u:p@[::1]:4318/a@b", "http://[::1]:4318/a@b"),
            ("collector:4317/a@b", "collector:4317/a@b"),
        ];
        for (raw, expected) in cases {
            assert_eq!(sanitize_url(raw), expected, "{raw}");
        }
    }

    #[test]
    fn redact_url_catches_normalised_forms_of_the_configured_url() {
        let secret = "alice:hunter2";
        let cases = [
            // (configured, how a library quotes it back)
            (
                "http://alice:hunter2@host:4317",
                "http://alice:hunter2@host:4317/",
            ),
            (
                "http://alice:hunter2@host:4317/",
                "http://alice:hunter2@host:4317",
            ),
            (
                "HTTP://alice:hunter2@Host:4317",
                "http://alice:hunter2@host:4317",
            ),
            (
                "http://alice:hunter2@host:80/v1",
                "http://alice:hunter2@host/v1",
            ),
            (
                "https://alice:hunter2@host/v1",
                "https://alice:hunter2@host:443/v1",
            ),
            (
                "https://alice:hunter2@HOST:443",
                "https://alice:hunter2@host/",
            ),
            (
                "http://alice:hunter2@host/a?token=t0p",
                "http://alice:hunter2@host:80/a?token=t0p",
            ),
            // A form nobody predicted still loses its userinfo.
            (
                "http://alice:hunter2@host:4317",
                "dns error for alice:hunter2@host.:4317",
            ),
        ];
        for (configured, quoted) in cases {
            let text = format!("invalid URI {quoted}. Reason: bad");
            let redacted = redact_url(&text, configured);
            assert!(
                !redacted.contains(secret),
                "{configured} / {quoted}: {redacted}"
            );
            assert!(
                !redacted.contains("t0p"),
                "{configured} / {quoted}: {redacted}"
            );
            assert!(redacted.contains("host"), "{redacted}");
            assert!(redacted.starts_with("invalid URI "), "{redacted}");
            assert!(redacted.ends_with(". Reason: bad"), "{redacted}");
        }
    }

    #[test]
    fn redact_url_without_credentials_changes_nothing_else() {
        assert_eq!(
            redact_url("cannot reach http://host:4317/ today", "http://host:4317"),
            "cannot reach http://host:4317/ today"
        );
    }
}

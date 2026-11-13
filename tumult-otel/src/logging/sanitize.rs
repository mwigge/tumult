//! Making values safe to log.

/// Removes the parts of a URL that can carry credentials — the userinfo
/// (`user:password@`), the query string and the fragment — so the result is
/// safe to log as `url.full`.
///
/// Works on anything URL-shaped, with or without a scheme, and never fails:
/// a value that is not a URL is returned without its query and fragment.
#[must_use]
pub fn sanitize_url(raw: &str) -> String {
    let without_query = raw.split(['?', '#']).next().unwrap_or(raw);
    let (scheme, rest) = match without_query.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, without_query),
    };
    let (authority, path) = match rest.find('/') {
        Some(slash) => rest.split_at(slash),
        None => (rest, ""),
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    match scheme {
        Some(scheme) => format!("{scheme}://{host}{path}"),
        None => format!("{host}{path}"),
    }
}

/// Replaces every occurrence of `url` in `text` with its
/// [sanitised](sanitize_url) form.
///
/// Error messages for a bad endpoint usually quote the endpoint verbatim;
/// this keeps its credentials out of the `error.message` field.
#[must_use]
pub fn redact_url(text: &str, url: &str) -> String {
    if url.is_empty() {
        return text.to_string();
    }
    text.replace(url, &sanitize_url(url))
}

#[cfg(test)]
mod tests {
    use super::{redact_url, sanitize_url};

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
}

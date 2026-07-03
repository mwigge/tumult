pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// XML-safe escaping. Extends [`html_escape`] with the apostrophe entity.
pub(crate) fn xml_escape(s: &str) -> String {
    html_escape(s).replace('\'', "&apos;")
}

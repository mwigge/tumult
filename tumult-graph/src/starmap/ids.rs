//! Node id segments. An id is `kind:segment/segment`, and an edge id is
//! `from|rel|to`, so a segment must never carry `/` or `|` of its own.

/// One id segment: `%`, `/` and `|` are percent-encoded, everything else
/// is kept, so two different names never share an id.
pub(super) fn seg(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '%' => out.push_str("%25"),
            '/' => out.push_str("%2F"),
            '|' => out.push_str("%7C"),
            _ => out.push(c),
        }
    }
    out
}

/// The environment segment. A missing environment is `-`, so an
/// environment that is literally named `-` is encoded.
pub(super) fn env_seg(env: Option<&str>) -> String {
    match env {
        None => "-".to_string(),
        Some("-") => "%2D".to_string(),
        Some(env) => seg(env),
    }
}

/// A slash-separated path (an org path, an article or gap id): the
/// slashes are structure and stay, `%` and `|` are encoded.
pub(super) fn path(raw: &str) -> String {
    raw.replace('%', "%25").replace('|', "%7C")
}

/// Whether an entity id has the agreed form: lower-case ASCII segments
/// joined by `-`, at least two of them, the first starting with a
/// letter, at most 63 characters.
pub(super) fn well_formed_entity(id: &str) -> bool {
    let alnum = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    let segments: Vec<&str> = id.split('-').collect();
    let starts_with_letter = id.as_bytes().first().is_some_and(u8::is_ascii_lowercase);
    id.len() <= 63 && segments.len() > 1 && starts_with_letter && segments.into_iter().all(alnum)
}

/// Of two spellings of one entity id, the one a node shows: the agreed
/// form when either has it, else the first in byte order, so the answer
/// never depends on which row was read first.
pub(super) fn shown_entity<'s>(held: &'s str, other: &'s str) -> &'s str {
    match (well_formed_entity(held), well_formed_entity(other)) {
        (true, false) => held,
        (false, true) => other,
        _ => held.min(other),
    }
}

#[cfg(test)]
mod tests {
    use super::{env_seg, path, seg, shown_entity, well_formed_entity};

    #[test]
    fn the_shown_spelling_does_not_depend_on_order() {
        for (a, b, shown) in [
            ("CIF-0042", "cif-0042", "cif-0042"),
            ("Cif-0042", "CIF-0042", "CIF-0042"),
            ("cif-0042", "cif-0042", "cif-0042"),
        ] {
            assert_eq!(shown_entity(a, b), shown);
            assert_eq!(shown_entity(b, a), shown);
        }
    }

    #[test]
    fn segments_never_carry_a_separator() {
        assert_eq!(seg("plain-name_1.2"), "plain-name_1.2");
        assert_eq!(seg("a/b|c%d"), "a%2Fb%7Cc%25d");
        assert_ne!(seg("a%2Fb"), seg("a/b"));
        assert_eq!(env_seg(None), "-");
        assert_eq!(env_seg(Some("-")), "%2D");
        assert_eq!(env_seg(Some("eu/west")), "eu%2Fwest");
        assert_eq!(path("platform/a|b/50%"), "platform/a%7Cb/50%25");
    }

    #[test]
    fn entity_id_form() {
        assert!(well_formed_entity("cif-0042"));
        assert!(well_formed_entity("a-b-c9"));
        assert!(!well_formed_entity(""));
        assert!(!well_formed_entity("cif"));
        assert!(!well_formed_entity("cif-"));
        assert!(!well_formed_entity("-cif"));
        assert!(!well_formed_entity("CIF-0042"));
        assert!(!well_formed_entity("0cif-1"));
        assert!(!well_formed_entity("cif_1-2"));
        assert!(well_formed_entity(&format!("c-{}", "0".repeat(61))));
        assert!(!well_formed_entity(&format!("c-{}", "0".repeat(62))));
    }
}

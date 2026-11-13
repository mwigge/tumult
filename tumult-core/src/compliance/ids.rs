//! Identifier forms used by the function register.
//!
//! `ict.entity.id` is a house identifier: lower-case ASCII, at least two
//! hyphen-separated groups, the first starting with a letter
//! (`^[a-z][a-z0-9]*(-[a-z0-9]+)+$`), at most 63 characters, opaque and
//! stable. A value that does not match is reported, never rewritten:
//! `CF-0042` → `cif-0042` is not a case fold, so no automatic repair can
//! be correct.
//!
//! Legal entities are keyed by LEI (ISO 17442): 20 upper-case
//! alphanumerics whose last two characters are ISO/IEC 7064 MOD 97-10
//! check digits.

/// The canonical form of an `ict.entity.id`, as shown to people. The
/// check itself is [`check_entity_id`], which also bounds the length.
pub const ENTITY_ID_PATTERN: &str = "^[a-z][a-z0-9]*(-[a-z0-9]+)+$";

/// Longest accepted `ict.entity.id`, in bytes.
pub const ENTITY_ID_MAX_LEN: usize = 63;

/// Resource attribute carrying the ICT entity id.
pub const ENTITY_ID_ATTR: &str = "ict.entity.id";

/// Resource attribute the ingest path sets to `"true"` next to an
/// `ict.entity.id` that does not match the canonical form. The original
/// value is kept untouched.
pub const ENTITY_ID_MALFORMED_ATTR: &str = "tumult.ict.entity.id.malformed";

/// Length of a LEI.
const LEI_LEN: usize = 20;

/// A malformed `ict.entity.id`, with the reason it was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityIdError {
    /// The offending value, exactly as received.
    pub value: String,
    /// Why it does not match the canonical form.
    pub reason: &'static str,
}

impl std::fmt::Display for EntityIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ict.entity.id '{}' is malformed: {} (expected lower-case groups joined by '-', \
             e.g. cif-0042)",
            self.value, self.reason
        )
    }
}

impl std::error::Error for EntityIdError {}

fn group_is_valid(group: &str) -> bool {
    !group.is_empty()
        && group
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn entity_id_fault(id: &str) -> Option<&'static str> {
    if id.is_empty() {
        return Some("empty");
    }
    if id.len() > ENTITY_ID_MAX_LEN {
        return Some("longer than 63 characters");
    }
    if !id.as_bytes()[0].is_ascii_lowercase() {
        return Some("must start with a lower-case letter");
    }
    let mut groups = id.split('-');
    let groups_valid = groups.by_ref().all(group_is_valid);
    if !groups_valid {
        return Some("only lower-case letters, digits and single hyphens are allowed");
    }
    if !id.contains('-') {
        return Some("needs at least two hyphen-separated groups");
    }
    None
}

/// Check an `ict.entity.id` against the canonical form.
///
/// # Errors
/// Returns the untouched value and the reason when it does not match.
pub fn check_entity_id(id: &str) -> Result<(), EntityIdError> {
    let Some(reason) = entity_id_fault(id) else {
        return Ok(());
    };
    Err(EntityIdError {
        value: id.to_string(),
        reason,
    })
}

/// Whether `id` is a canonical `ict.entity.id`.
#[must_use]
pub fn entity_id_is_valid(id: &str) -> bool {
    entity_id_fault(id).is_none()
}

/// Whether `lei` is a well-formed LEI: 20 upper-case alphanumerics with
/// valid MOD 97-10 check digits.
#[must_use]
pub fn lei_is_valid(lei: &str) -> bool {
    if lei.len() != LEI_LEN {
        return false;
    }
    let mut remainder: u32 = 0;
    for byte in lei.bytes() {
        let value = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'A'..=b'Z' => u32::from(byte - b'A') + 10,
            _ => return false,
        };
        let shift = if value >= 10 { 100 } else { 10 };
        remainder = (remainder * shift + value) % 97;
    }
    remainder == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_entity_ids_are_accepted() {
        for id in ["cif-0042", "cif-0050", "pay-ledger-01", "a-1", "x9-y"] {
            assert!(entity_id_is_valid(id), "{id} must be valid");
        }
    }

    #[test]
    fn malformed_entity_ids_are_rejected_not_folded() {
        for id in [
            "CF-0042",
            "Cif-0042",
            "cif_0042",
            "cif",
            "0042-cif",
            "cif-",
            "-cif",
            "cif--0042",
            "cif-0042 ",
            "",
            "cif.0042",
        ] {
            assert!(!entity_id_is_valid(id), "{id:?} must be rejected");
        }
        // The check never rewrites its input: an upper-case id stays wrong.
        assert_eq!(check_entity_id("CF-0042").unwrap_err().value, "CF-0042");
    }

    #[test]
    fn entity_id_length_is_capped_at_63() {
        let ok = format!("a-{}", "0".repeat(ENTITY_ID_MAX_LEN - 2));
        assert_eq!(ok.len(), 63);
        assert!(entity_id_is_valid(&ok));
        let long = format!("a-{}", "0".repeat(ENTITY_ID_MAX_LEN - 1));
        assert!(!entity_id_is_valid(&long));
    }

    #[test]
    fn lei_needs_twenty_alphanumerics_and_valid_check_digits() {
        assert!(lei_is_valid("529900T8BM49AURSDO55"));
        assert!(lei_is_valid("506700GE1G29325QX363"));
        assert!(!lei_is_valid("529900T8BM49AURSDO56"), "wrong check digits");
        assert!(!lei_is_valid("529900T8BM49AURSDO5"), "19 characters");
        assert!(!lei_is_valid("529900t8bm49aursdo55"), "lower case");
        assert!(!lei_is_valid("529900T8BM49AURSDO5-"), "punctuation");
    }
}

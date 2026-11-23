//! One form for a DORA requirement id.
//!
//! The id of the requirement registry is the one form: `dora.art24.6` for
//! a row, `dora.art24` for an article the registry has rows of, and
//! `rts1774.art25` for a row of the RTS 2024/1774. Records and experiment
//! files written before named the same requirement in other forms:
//! `DORA Art. 24(6)` (the citation text the Star Map read), `DORA-Art24`
//! (the run block of the docs) and a bare `Art. 24` under a mapping that
//! lists DORA. [`requirement_id`] reads all of them and gives the registry
//! id, so the score, the Star Map and a run's regulatory block agree on
//! what a reference names.
//!
//! On write, [`check_reference`] (a manual record) and
//! [`check_run_requirement`] (an experiment file) refuse a DORA reference
//! the registry does not know and name the nearest registry id. A
//! reference to another framework (`ISO 27001 A.5.30`) is outside the
//! registry and is kept as written.

use std::fmt;

use super::citations::{Citation, CITATIONS};
use super::framework::ComplianceFramework;
use super::requirements::{ARTICLES, REQUIREMENTS};

/// Registry ids of DORA articles start with this.
const DORA_ID: &str = "dora.art";
/// Registry ids of the RTS 2024/1774 rows start with this.
const RTS_ID: &str = "rts1774.art";
/// The skeleton of the RTS 2024/1774 label, as in `RTS 2024/1774 Art. 25`.
const RTS_LABEL: &str = "rts20241774";

/// A DORA reference the registry does not know, with the registry id
/// nearest to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownRequirement {
    /// The reference as it was written.
    pub reference: String,
    /// The nearest registry id, when one is near enough to name.
    pub suggestion: Option<String>,
}

impl fmt::Display for UnknownRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' is not a requirement id of the DORA registry",
            self.reference
        )?;
        match &self.suggestion {
            Some(id) => write!(f, "; did you mean '{id}'?"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for UnknownRequirement {}

/// What a written reference names.
enum Parsed {
    /// A reference in the registry form, lower case.
    Registry(String),
    /// A DORA reference in another form: the skeleton after `DORA`, such as
    /// `art24(6)`.
    Dora(String),
    /// An RTS 2024/1774 reference: the skeleton after its label, such as
    /// `art25`.
    Rts(String),
    /// A reference to another framework of the citation catalogue.
    Other,
    /// No framework named: the skeleton of the whole reference.
    Bare(String),
}

/// The comparison skeleton of a reference: lower case, letters, digits and
/// brackets only. `DORA Art. 24(6)` and `dora-art24(6)` both give
/// `doraart24(6)`.
fn skeleton(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '(' | ')'))
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The skeletons of the labels of the frameworks other than DORA, as in
/// `NIS2`, `PCI-DSS`, `ISO 27001` or `ISO/IEC 27001`.
fn other_labels() -> impl Iterator<Item = String> {
    ComplianceFramework::ALL
        .into_iter()
        .filter(|f| *f != ComplianceFramework::Dora)
        .flat_map(|f| {
            let report = skeleton(f.as_report_str());
            let iec = report.strip_prefix("iso").map(|n| format!("isoiec{n}"));
            [Some(skeleton(f.name())), Some(report), iec]
        })
        .flatten()
}

fn parse(reference: &str) -> Parsed {
    let lower = reference.trim().to_ascii_lowercase();
    if lower.starts_with(DORA_ID) || lower.starts_with(RTS_ID) {
        return Parsed::Registry(lower);
    }
    let skel = skeleton(reference);
    if let Some(rest) = skel.strip_prefix(RTS_LABEL) {
        return Parsed::Rts(rest.to_owned());
    }
    if let Some(rest) = skel.strip_prefix("dora") {
        return Parsed::Dora(rest.to_owned());
    }
    if other_labels().any(|label| skel.starts_with(&label)) {
        return Parsed::Other;
    }
    Parsed::Bare(skel)
}

/// The keys of the DORA articles of the registry: `5`, `6`, `9`, …
fn dora_articles() -> impl Iterator<Item = &'static str> {
    ARTICLES
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| key.chars().all(|c| c.is_ascii_digit()))
}

/// Every id the registry knows: its rows in registry order, then its DORA
/// articles.
fn known_ids() -> impl Iterator<Item = String> {
    REQUIREMENTS
        .iter()
        .map(|r| r.id.to_owned())
        .chain(dora_articles().map(|key| format!("{DORA_ID}{key}")))
}

fn is_known(id: &str) -> bool {
    known_ids().any(|known| known == id)
}

/// The registry id of a DORA citation skeleton such as `art24(6)`: the
/// first row with that citation, else the article when the skeleton names
/// only an article of the registry.
fn dora_exact(rest: &str) -> Option<String> {
    let row = REQUIREMENTS
        .iter()
        .filter(|r| r.id.starts_with(DORA_ID))
        .find(|r| skeleton(r.cite) == rest);
    if let Some(row) = row {
        return Some(row.id.to_owned());
    }
    let article = rest.strip_prefix("art")?;
    dora_articles()
        .find(|key| *key == article)
        .map(|key| format!("{DORA_ID}{key}"))
}

/// The article of the registry a DORA skeleton such as `art24(2)` falls
/// under, for a paragraph the registry has no row of.
fn dora_article_of(rest: &str) -> Option<String> {
    let digits: String = rest
        .strip_prefix("art")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    dora_articles()
        .find(|key| *key == digits)
        .map(|key| format!("{DORA_ID}{key}"))
}

/// The article of a registry id such as `dora.art24.2`.
fn registry_article_of(id: &str) -> Option<String> {
    let rest = id.strip_prefix(DORA_ID)?;
    dora_article_of(&format!("art{rest}"))
}

fn rts_exact(rest: &str) -> Option<String> {
    let want = format!("{RTS_LABEL}{rest}");
    REQUIREMENTS
        .iter()
        .find(|r| r.id.starts_with(RTS_ID) && skeleton(r.cite) == want)
        .map(|r| r.id.to_owned())
}

/// The registry id a DORA reference names exactly; `None` for anything
/// else, also for a reference to a paragraph the registry has no row of.
fn exact(parsed: &Parsed) -> Option<String> {
    match parsed {
        Parsed::Registry(id) => is_known(id).then(|| id.clone()),
        Parsed::Dora(rest) => dora_exact(rest),
        Parsed::Rts(rest) => rts_exact(rest),
        Parsed::Other | Parsed::Bare(_) => None,
    }
}

/// The registry id a DORA reference names, in any form Tumult has read:
/// the registry form, the citation text (`DORA Art. 24(6)`,
/// `RTS 2024/1774 Art. 25`) and the run block form (`DORA-Art24`). A
/// paragraph the registry has no row of reads as its article, so an old
/// record still shows on the Star Map. `None` for a reference to another
/// framework, a reference with no framework, and a DORA article the
/// registry does not hold.
///
/// This is the one mapping the score, the Star Map and the readers of a
/// run's regulatory block use.
#[must_use]
pub fn requirement_id(reference: &str) -> Option<String> {
    let parsed = parse(reference);
    if let Some(id) = exact(&parsed) {
        return Some(id);
    }
    match parsed {
        Parsed::Registry(id) => registry_article_of(&id),
        Parsed::Dora(rest) => dora_article_of(&rest),
        Parsed::Rts(_) | Parsed::Other | Parsed::Bare(_) => None,
    }
}

/// Whether a framework label of a regulatory block names DORA.
fn names_dora(framework: &str) -> bool {
    skeleton(framework) == "dora"
}

/// A bare id of a run block (`Art. 25`) as the DORA reference it is when
/// the block lists DORA.
fn under_dora(frameworks: &[String], id: &str) -> Option<String> {
    frameworks
        .iter()
        .any(|f| names_dora(f))
        .then(|| format!("DORA {id}"))
}

/// The registry id a requirement of a run's regulatory block names: a
/// DORA reference in any form, or a bare id such as `Art. 25` when the
/// block lists DORA among its frameworks. `None` when it names no DORA
/// requirement the registry holds.
#[must_use]
pub fn run_requirement_id(frameworks: &[String], id: &str) -> Option<String> {
    if let Some(found) = requirement_id(id) {
        return Some(found);
    }
    if !matches!(parse(id), Parsed::Bare(_)) {
        return None;
    }
    requirement_id(&under_dora(frameworks, id)?)
}

/// The edit distance of two ids, in characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The registry form a reference would have: `DORA Art. 24(7)` and a bare
/// `Art. 24(7)` give `dora.art24.7`.
fn as_registry_form(parsed: &Parsed) -> String {
    let dotted = |rest: &str| rest.replace('(', ".").replace(')', "");
    match parsed {
        Parsed::Registry(id) => id.clone(),
        Parsed::Rts(rest) => format!("rts1774.{}", dotted(rest)),
        Parsed::Dora(rest) | Parsed::Bare(rest) => {
            let rest = rest.strip_prefix("art").unwrap_or(rest);
            format!("{DORA_ID}{}", dotted(rest))
        }
        Parsed::Other => String::new(),
    }
}

/// The length of the text two ids start with.
fn shared_start(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// The known registry id nearest to `reference`; on a tie, the one that
/// starts with more of it (`dora.art12.9` is nearer `dora.art12.1` than
/// `dora.art11.9`), then the first in registry order.
fn nearest(parsed: &Parsed) -> Option<String> {
    let want = as_registry_form(parsed);
    known_ids().min_by_key(|id| {
        (
            distance(&want, id),
            std::cmp::Reverse(shared_start(&want, id)),
        )
    })
}

fn unknown(reference: &str, parsed: &Parsed) -> UnknownRequirement {
    UnknownRequirement {
        reference: reference.trim().to_owned(),
        suggestion: nearest(parsed),
    }
}

/// The write check of one framework reference of a manual record. A DORA
/// reference must name a registry row or article exactly, in the registry
/// form or a form [`requirement_id`] reads. A reference that names
/// another framework of the citation catalogue is outside the registry
/// and passes. Anything else is refused.
///
/// # Errors
///
/// [`UnknownRequirement`] with the nearest registry id, for a DORA
/// reference the registry does not know and for a reference that names no
/// framework (such as a bare `Art. 24(6)`).
pub fn check_reference(reference: &str) -> Result<(), UnknownRequirement> {
    let parsed = parse(reference);
    if matches!(parsed, Parsed::Other) || exact(&parsed).is_some() {
        return Ok(());
    }
    Err(unknown(reference, &parsed))
}

/// The write check of one requirement id of a run's regulatory block. A
/// DORA reference in any form must name a registry row or article
/// exactly. A bare id (`Art. 25`) is checked as DORA when DORA is the only
/// framework the block lists; with other frameworks listed it may be one
/// of theirs, and it passes.
///
/// # Errors
///
/// [`UnknownRequirement`] with the nearest registry id.
pub fn check_run_requirement(frameworks: &[String], id: &str) -> Result<(), UnknownRequirement> {
    let parsed = parse(id);
    let dora_only = !frameworks.is_empty() && frameworks.iter().all(|f| names_dora(f));
    let checked = match &parsed {
        Parsed::Other => return Ok(()),
        Parsed::Bare(_) if !dora_only => return Ok(()),
        Parsed::Bare(rest) => exact(&Parsed::Dora(rest.clone())),
        _ => exact(&parsed),
    };
    match checked {
        Some(_) => Ok(()),
        None => Err(unknown(id, &parsed)),
    }
}

/// The DORA citation of the catalogue whose control id is `control`.
fn dora_citation(control: &str) -> Option<&'static Citation> {
    CITATIONS
        .iter()
        .find(|c| c.framework == ComplianceFramework::Dora && c.control_id == control)
}

/// The citation of the Star Map article a registry id falls under: the
/// citation of the row's own paragraph when the catalogue has one
/// (`dora.art24.6` is `Art. 24(6)`), else the one of its article
/// (`dora.art25.1` is `Art. 25`). `None` when the catalogue has neither.
#[must_use]
pub fn citation_of(id: &str) -> Option<&'static Citation> {
    if let Some(row) = REQUIREMENTS.iter().find(|r| r.id == id) {
        return dora_citation(row.cite).or_else(|| dora_citation(&format!("Art. {}", row.article)));
    }
    let article = id.strip_prefix(DORA_ID)?;
    dora_citation(&format!("Art. {article}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_id_names_itself() {
        for id in ["dora.art24.6", "dora.art11.6a", "dora.art11.6.switchover"] {
            assert_eq!(requirement_id(id).as_deref(), Some(id));
        }
        assert_eq!(
            requirement_id(" DORA.ART24.6 ").as_deref(),
            Some("dora.art24.6")
        );
        assert_eq!(
            requirement_id("rts1774.art25").as_deref(),
            Some("rts1774.art25")
        );
        assert_eq!(requirement_id("dora.art24").as_deref(), Some("dora.art24"));
    }

    #[test]
    fn the_old_forms_read_as_the_registry_id() {
        for (old, id) in [
            ("DORA Art. 24(6)", "dora.art24.6"),
            ("DORA Art. 5(2)", "dora.art5.2"),
            ("DORA Art. 11(6)(a)", "dora.art11.6a"),
            ("DORA Art. 16(1)(f)", "dora.art16.1.f"),
            ("DORA Art. 12(3)", "dora.art12.3"),
            ("DORA Art. 25", "dora.art25"),
            ("DORA-Art24", "dora.art24"),
            ("dora art 12(1)", "dora.art12.1"),
            ("RTS 2024/1774 Art. 25", "rts1774.art25"),
        ] {
            assert_eq!(requirement_id(old).as_deref(), Some(id), "{old}");
        }
    }

    #[test]
    fn a_paragraph_the_registry_has_no_row_of_reads_as_its_article() {
        assert_eq!(
            requirement_id("DORA Art. 24(2)").as_deref(),
            Some("dora.art24")
        );
        assert_eq!(
            requirement_id("dora.art24.2").as_deref(),
            Some("dora.art24")
        );
        assert_eq!(requirement_id("DORA Art. 99"), None);
    }

    #[test]
    fn another_framework_or_no_framework_names_no_registry_id() {
        for reference in ["ISO 27001 A.5.30", "NIS2 Art. 21(2)(c)", "Art. 24(6)", ""] {
            assert_eq!(requirement_id(reference), None, "{reference}");
        }
    }

    #[test]
    fn a_bare_id_of_a_run_block_is_dora_when_the_block_lists_dora() {
        let dora = vec!["DORA".to_owned(), "NIS2".to_owned()];
        let nis2 = vec!["NIS2".to_owned()];
        assert_eq!(
            run_requirement_id(&dora, "Art. 25").as_deref(),
            Some("dora.art25")
        );
        assert_eq!(
            run_requirement_id(&dora, "Art. 12(1)").as_deref(),
            Some("dora.art12.1")
        );
        assert_eq!(run_requirement_id(&nis2, "Art. 25"), None);
        assert_eq!(
            run_requirement_id(&nis2, "dora.art25.1").as_deref(),
            Some("dora.art25.1")
        );
        assert_eq!(run_requirement_id(&dora, "Art. 21(2)(c)"), None);
    }

    #[test]
    fn a_manual_reference_passes_when_the_registry_or_another_framework_knows_it() {
        for reference in [
            "dora.art24.6",
            "dora.art24",
            "DORA Art. 24(6)",
            "RTS 2024/1774 Art. 40",
            "ISO 27001 A.5.30",
            "ISO/IEC 27001 A.8.13",
            "NIS2 Art. 21(2)(c)",
            "PCI-DSS Req. 12.10.2",
        ] {
            assert_eq!(check_reference(reference), Ok(()), "{reference}");
        }
    }

    #[test]
    fn an_unknown_manual_reference_is_refused_with_the_nearest_registry_id() {
        for (typo, nearest) in [
            ("dora.art24.7", "dora.art24.1"),
            ("dora.art12.9", "dora.art12.1"),
            ("dora.art24-6", "dora.art24.6"),
            ("DORA Art. 24(2)", "dora.art24.1"),
            ("DORA Art. 99", "dora.art9"),
            ("Art. 24(6)", "dora.art24.6"),
        ] {
            let err = check_reference(typo).unwrap_err();
            assert_eq!(err.reference, typo);
            assert_eq!(err.suggestion.as_deref(), Some(nearest), "{typo}");
        }
        assert_eq!(
            check_reference("dora.art24.7").unwrap_err().to_string(),
            "'dora.art24.7' is not a requirement id of the DORA registry; \
             did you mean 'dora.art24.1'?"
        );
    }

    #[test]
    fn a_run_block_id_is_checked_as_dora_when_it_is_dora() {
        let dora = vec!["DORA".to_owned()];
        let both = vec!["DORA".to_owned(), "NIS2".to_owned()];
        let nis2 = vec!["NIS2".to_owned()];
        for ok in [
            "Art. 11",
            "Art. 25",
            "Art. 24(6)",
            "DORA-Art24",
            "dora.art12.1",
        ] {
            assert_eq!(check_run_requirement(&dora, ok), Ok(()), "{ok}");
        }
        let err = check_run_requirement(&dora, "Art. 99").unwrap_err();
        assert_eq!(err.suggestion.as_deref(), Some("dora.art9"));
        let err = check_run_requirement(&nis2, "dora.art24.7").unwrap_err();
        assert_eq!(err.suggestion.as_deref(), Some("dora.art24.1"));
        assert_eq!(check_run_requirement(&both, "Art. 21(2)(c)"), Ok(()));
        assert_eq!(check_run_requirement(&nis2, "Art. 21(2)(c)"), Ok(()));
        assert!(check_run_requirement(&both, "DORA Art. 99").is_err());
    }

    #[test]
    fn a_registry_id_falls_under_the_star_map_article_of_its_paragraph_or_article() {
        let control = |id: &str| citation_of(id).map(|c| c.control_id);
        assert_eq!(control("dora.art24.6"), Some("Art. 24(6)"));
        assert_eq!(control("dora.art25.1"), Some("Art. 25"));
        assert_eq!(control("dora.art11.6a"), Some("Art. 11"));
        assert_eq!(control("dora.art12"), Some("Art. 12"));
        assert_eq!(control("dora.art5.2"), None);
        assert_eq!(control("rts1774.art25"), None);
    }
}

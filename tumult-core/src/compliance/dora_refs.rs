//! Existence check for DORA article and paragraph references such as
//! `DORA Art. 24(6)`, so a citation of a paragraph that is not in the
//! regulation fails a test instead of reaching an evidence pack.

/// Prefix every DORA article reference starts with.
const DORA_REF_PREFIX: &str = "DORA Art. ";

/// Paragraph table for the articles of Regulation (EU) 2022/2554 the citation
/// registry and the seeded manual records cite, as `(article, paragraphs)`.
///
/// Articles 24 and 25 carry their full paragraph count (six and three). For
/// Articles 11, 12 and 26 the number is the highest paragraph confirmed
/// against the Official Journal text in the review of 2026-11-13; raise it
/// only after reading the paragraph in the official text. An article that is
/// not listed is treated as not verified, so a reference to it fails.
const DORA_PARAGRAPHS: &[(u32, u32)] = &[(11, 10), (12, 7), (24, 6), (25, 3), (26, 8)];

/// A parsed `DORA Art. <article>` or `DORA Art. <article>(<paragraph>)`
/// reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoraRef {
    /// Article number.
    pub article: u32,
    /// Paragraph number, when the reference names one.
    pub paragraph: Option<u32>,
}

/// Parse `DORA Art. 24` or `DORA Art. 24(6)`. Returns `None` for any other
/// shape, including points below a paragraph and trailing text.
#[must_use]
pub fn parse_dora_ref(framework_ref: &str) -> Option<DoraRef> {
    let rest = framework_ref.strip_prefix(DORA_REF_PREFIX)?;
    let Some((article, tail)) = rest.split_once('(') else {
        return Some(DoraRef {
            article: rest.parse().ok()?,
            paragraph: None,
        });
    };
    let paragraph = tail.strip_suffix(')')?;
    Some(DoraRef {
        article: article.parse().ok()?,
        paragraph: Some(paragraph.parse().ok()?),
    })
}

/// Number of paragraphs recorded for a DORA article in the paragraph table,
/// or `None` when the article is not in the table.
#[must_use]
pub fn dora_paragraph_count(article: u32) -> Option<u32> {
    DORA_PARAGRAPHS
        .iter()
        .find(|(listed, _)| *listed == article)
        .map(|(_, paragraphs)| *paragraphs)
}

/// Whether `framework_ref` is a DORA article reference whose article, and
/// paragraph when given, exist in the paragraph table.
#[must_use]
pub fn dora_ref_exists(framework_ref: &str) -> bool {
    let Some(parsed) = parse_dora_ref(framework_ref) else {
        return false;
    };
    let Some(count) = dora_paragraph_count(parsed.article) else {
        return false;
    };
    parsed
        .paragraph
        .is_none_or(|paragraph| (1..=count).contains(&paragraph))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compliance::{ComplianceFramework, CITATIONS};

    fn paragraph_ref(article: u32, paragraph: u32) -> String {
        format!("DORA Art. {article}({paragraph})")
    }

    #[test]
    fn parses_article_only_and_article_with_paragraph() {
        assert_eq!(
            parse_dora_ref("DORA Art. 25"),
            Some(DoraRef {
                article: 25,
                paragraph: None
            })
        );
        assert_eq!(
            parse_dora_ref(&paragraph_ref(11, 6)),
            Some(DoraRef {
                article: 11,
                paragraph: Some(6)
            })
        );
    }

    #[test]
    fn rejects_strings_that_are_not_a_dora_article_reference() {
        for bad in [
            "",
            "DORA",
            "DORA Art. ",
            "DORA Art. x",
            "DORA Art. 24(",
            "DORA Art. 24()",
            "DORA Art. 24(six)",
            "DORA Art. 24(6) and more",
            "ISO 27001 A.5.30",
            "Art. 24(6)",
        ] {
            assert_eq!(parse_dora_ref(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn article_24_has_six_paragraphs() {
        assert_eq!(dora_paragraph_count(24), Some(6));
        for paragraph in 1..=6 {
            assert!(dora_ref_exists(&paragraph_ref(24, paragraph)));
        }
        assert!(!dora_ref_exists(&paragraph_ref(24, 7)));
        assert!(!dora_ref_exists(&paragraph_ref(24, 0)));
    }

    #[test]
    fn an_article_missing_from_the_table_does_not_exist() {
        assert_eq!(dora_paragraph_count(99), None);
        assert!(!dora_ref_exists("DORA Art. 99"));
        assert!(!dora_ref_exists(&paragraph_ref(99, 1)));
    }

    #[test]
    fn every_dora_registry_ref_cites_an_existing_article_and_paragraph() {
        let refs: Vec<String> = CITATIONS
            .iter()
            .filter(|c| c.framework == ComplianceFramework::Dora)
            .map(|c| format!("DORA {}", c.control_id))
            .collect();
        assert!(!refs.is_empty(), "the registry must hold DORA rows");
        for framework_ref in refs {
            assert!(
                framework_ref.starts_with("DORA Art."),
                "{framework_ref:?} is not an article reference"
            );
            assert!(
                dora_ref_exists(&framework_ref),
                "{framework_ref:?} cites an article or paragraph missing from the paragraph table"
            );
        }
    }
}

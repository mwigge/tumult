//! A total order over version strings: what reads as a version sorts by
//! its numbers, everything else is text and sorts after every version.

use std::cmp::Ordering;

/// A run of digits as a number of any size: fewer digits (leading zeros
/// aside) is less, the same count compares digit by digit. Nothing is
/// parsed into a machine word, so no number is too long to order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Number {
    digits: usize,
    text: String,
}

impl Number {
    /// `None` unless `text` is one or more ASCII digits.
    fn read(text: &str) -> Option<Self> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let significant = text.trim_start_matches('0');
        Some(Self {
            digits: significant.len(),
            text: significant.to_string(),
        })
    }

    fn is_zero(&self) -> bool {
        self.digits == 0
    }
}

/// One identifier of a pre-release tag: numbers sort below text.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Number(Number),
    Text(String),
}

impl Part {
    /// `None` unless `text` is one or more of `[0-9A-Za-z-]`.
    fn read(text: &str) -> Option<Self> {
        let fits = |b: u8| b.is_ascii_alphanumeric() || b == b'-';
        if text.is_empty() || !text.bytes().all(fits) {
            return None;
        }
        Some(Number::read(text).map_or_else(|| Self::Text(text.to_string()), Self::Number))
    }
}

/// What follows the numbers: a pre-release sorts below its release, and a
/// tag that is the start of another below the longer one.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Pre(Vec<Part>),
    Release,
}

/// What a version string is ordered by. The variants are the two classes,
/// in their order: everything that reads as a version, then all other
/// text. Two strings are compared inside one class only, which is what
/// makes the order total.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// Dotted numbers without their trailing zeros (`1.2` and `1.2.0` are
    /// the same number), then the stage.
    Version(Vec<Number>, Stage),
    Text,
}

/// The fewest dotted numbers a version has: one number alone is a date,
/// a build counter or a sha that happens to be all digits.
const MIN_NUMBERS: usize = 2;

impl Rank {
    fn of(text: &str) -> Self {
        Self::version(text).unwrap_or(Self::Text)
    }

    /// `None` unless `text` is an optional `v`, two or more dotted
    /// numbers, an optional `-` pre-release of dotted identifiers and an
    /// optional `+` build tag (which does not rank).
    fn version(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text
            .split_once('+')
            .map_or(text, |(version, _build)| version);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = core
            .split('.')
            .map(Number::read)
            .collect::<Option<Vec<_>>>()?;
        if numbers.len() < MIN_NUMBERS {
            return None;
        }
        while numbers.last().is_some_and(Number::is_zero) {
            numbers.pop();
        }
        let stage = match pre {
            None => Stage::Release,
            Some(tag) => Stage::Pre(tag.split('.').map(Part::read).collect::<Option<_>>()?),
        };
        Some(Self::Version(numbers, stage))
    }
}

/// Order two version strings. A string that reads as a version — an
/// optional leading `v`, two or more dotted numbers, an optional
/// pre-release and build tag — sorts before one that does not (a sha, a
/// date, a branch name). Versions compare by their numbers (`1.10.0`
/// above `1.9.0`, however long the numbers), then a pre-release below its
/// release with its identifiers compared as semver compares them; build
/// metadata does not rank. Everything the rank leaves equal, and all
/// other text, is ordered by the string itself, so the order is total:
/// only the same string compares equal.
#[must_use]
pub fn version_cmp(a: &str, b: &str) -> Ordering {
    Rank::of(a).cmp(&Rank::of(b)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_their_numbers_before_their_text() {
        use Ordering::{Equal, Greater, Less};
        assert_eq!(version_cmp("1.10.0", "1.9.0"), Greater);
        assert_eq!(version_cmp("1.9.0", "1.10.0"), Less);
        assert_eq!(version_cmp("v2.0.0", "1.99.99"), Greater);
        assert_eq!(version_cmp("2.0", "1.9.9"), Greater);
        assert_eq!(version_cmp("1.2.3", "1.2.3"), Equal);
        // A pre-release sorts below its release, and its parts by number.
        assert_eq!(version_cmp("1.0.0-rc.1", "1.0.0"), Less);
        assert_eq!(version_cmp("1.0.0-rc.10", "1.0.0-rc.2"), Greater);
        assert_eq!(version_cmp("1.0.0-alpha", "1.0.0-alpha.1"), Less);
        assert_eq!(version_cmp("1.0.0-1", "1.0.0-alpha"), Less);
        // Build metadata does not order; the text breaks the tie so the
        // order stays total.
        assert_eq!(version_cmp("1.0.0+b2", "1.0.1+b1"), Less);
        assert_eq!(version_cmp("1.0.0+b2", "1.0.0+b1"), Greater);
        assert_eq!(version_cmp("1.2", "1.2.0"), Less);
        // Anything that does not read as a version is text, and all text
        // sorts above every version: a sha, a date, a bare number, nothing.
        assert_eq!(version_cmp("main-abd", "main-abc"), Greater);
        assert_eq!(version_cmp("2024-06-01", "9.0.0"), Greater);
        assert_eq!(version_cmp("0a1b2c3", "9.0.0"), Greater);
        assert_eq!(version_cmp("1234567", "9.0.0"), Greater);
        assert_eq!(version_cmp("", "1.0.0"), Greater);
        assert_eq!(version_cmp("1.0.0", "main"), Less);
    }

    #[test]
    fn numbers_too_long_for_a_machine_word_still_compare_as_numbers() {
        use Ordering::{Greater, Less};
        let huge = "1.99999999999999999999999999999999999999.0";
        let huger = "1.100000000000000000000000000000000000000.0";
        assert_eq!(version_cmp(huge, huger), Less);
        assert_eq!(version_cmp(huge, "1.2.0"), Greater);
        assert_eq!(
            version_cmp("1.0.0-99999999999999999999999", "1.0.0-2"),
            Greater
        );
        // Leading zeros do not make a number greater.
        assert_eq!(version_cmp("1.007.0", "1.8.0"), Less);
    }

    /// Versions of every kind the order has to place: releases, a `v`
    /// prefix, pre-releases, build metadata, equal numbers spelled
    /// differently, numbers past `u64`, shas, dates, branch names, nothing.
    const CORPUS: [&str; 30] = [
        "1.0.0",
        "v1.0.0",
        "1.0",
        "1.0.0.0",
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0-1",
        "1.0.0-",
        "1.0.0+build.5",
        "1.0.0+build.10",
        "1.0.0-rc.1+sha.5114f85",
        "1.10.0",
        "1.9.0",
        "01.9.0",
        "2.0.0",
        "18446744073709551616.0.0",
        "18446744073709551615.0.0",
        "99999999999999999999999999.1",
        "2024-06-01",
        "2024.06.01",
        "20240601",
        "0a1b2c3",
        "5114f85",
        "main",
        "",
        "v",
    ];

    #[test]
    fn the_version_order_is_total_over_a_mixed_corpus() {
        use Ordering::Equal;
        for a in CORPUS {
            assert_eq!(version_cmp(a, a), Equal, "{a:?} equals itself");
            for b in CORPUS {
                let ab = version_cmp(a, b);
                assert_eq!(ab, version_cmp(b, a).reverse(), "antisymmetry: {a:?} {b:?}");
                assert_eq!(ab == Equal, a == b, "only equal text is equal: {a:?} {b:?}");
                for c in CORPUS {
                    if ab.is_le() && version_cmp(b, c).is_le() {
                        assert!(version_cmp(a, c).is_le(), "transitivity: {a:?} {b:?} {c:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn sorting_any_permutation_of_the_corpus_gives_one_order() {
        let mut expected = CORPUS.to_vec();
        expected.sort_by(|a, b| version_cmp(a, b));
        // Every version sorts before all text.
        let reads = |v: &str| version_cmp(v, "0.0").is_ge() && version_cmp(v, "").is_lt();
        let first_text = expected.iter().position(|v| !reads(v)).unwrap();
        assert!(
            expected[first_text..].iter().all(|v| !reads(v)),
            "{expected:?}"
        );
        assert_eq!(expected[first_text - 1], "99999999999999999999999999.1");
        assert_eq!(first_text, 22, "{expected:?}");
        // Rotations, the reverse, and a few hundred seeded shuffles.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).unwrap_or(0) % bound
        };
        let mut permutations: Vec<Vec<&str>> = (0..CORPUS.len())
            .map(|by| {
                let mut rotated = CORPUS.to_vec();
                rotated.rotate_left(by);
                rotated
            })
            .collect();
        permutations.push(CORPUS.iter().rev().copied().collect());
        for _ in 0..300 {
            let mut shuffled = CORPUS.to_vec();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, next(i + 1));
            }
            permutations.push(shuffled);
        }
        for mut permutation in permutations {
            permutation.sort_by(|a, b| version_cmp(a, b));
            assert_eq!(permutation, expected);
        }
    }
}

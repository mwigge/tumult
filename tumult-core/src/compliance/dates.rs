//! Date arithmetic backing the citation staleness audit.

use super::citations::{Citation, CITATIONS};

/// Parse an ISO-8601 `YYYY-MM-DD` date into `(year, month)`. Returns `None`
/// on any malformed input.
#[must_use]
pub fn parse_year_month(date: &str) -> Option<(i64, u32)> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let _day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month))
}

/// Current `(year, month)` in UTC, derived from the system clock. Used by the
/// staleness audit so it reflects the real calendar date at check time.
#[must_use]
pub fn current_year_month() -> (i64, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    #[allow(clippy::cast_possible_wrap)]
    let days = (secs / 86_400) as i64;
    let (y, m, _d) = civil_from_days(days);
    (y, m)
}

/// Howard Hinnant's `civil_from_days`: convert days since the Unix epoch
/// (1970-01-01) into a proleptic-Gregorian `(year, month, day)`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe.cast_signed() + era * 400; // yoe ∈ [0, 399], no wrap
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// Whole months from `last_verified` to `(now_year, now_month)`. Negative if
/// the citation date is in the future. Returns `None` on a malformed date.
#[must_use]
pub fn months_since_verified(last_verified: &str, now: (i64, u32)) -> Option<i64> {
    let (vy, vm) = parse_year_month(last_verified)?;
    let verified_months = vy * 12 + i64::from(vm - 1);
    let now_months = now.0 * 12 + i64::from(now.1 - 1);
    Some(now_months - verified_months)
}

/// Every citation older than `max_age_months` as of `now`, i.e. due for
/// re-verification against its official source.
#[must_use]
pub fn stale_citations(now: (i64, u32), max_age_months: i64) -> Vec<&'static Citation> {
    CITATIONS
        .iter()
        .filter(|c| match months_since_verified(c.last_verified, now) {
            Some(age) => age > max_age_months,
            None => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_since_verified_math() {
        assert_eq!(months_since_verified("2025-01-15", (2026, 7)), Some(18));
        assert_eq!(months_since_verified("2026-07-01", (2026, 7)), Some(0));
        assert_eq!(months_since_verified("2026-08-01", (2026, 7)), Some(-1));
        assert_eq!(months_since_verified("not-a-date", (2026, 7)), None);
    }

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}

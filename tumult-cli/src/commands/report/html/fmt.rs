//! How the report writes the values of a journal, and the HTML it puts
//! them in.

use std::time::Duration;

use chrono::DateTime;

pub(super) use super::super::escape::html_escape as text;

/// A number as short as it can be written: three decimals at most, and
/// none that say nothing.
pub(in crate::commands) fn number(value: f64) -> String {
    let written = format!("{value:.3}");
    let short = written.trim_end_matches('0').trim_end_matches('.');
    if short == "-0" {
        return "0".to_string();
    }
    short.to_string()
}

/// A time of `value` seconds, in the unit that reads best.
pub(super) fn seconds(value: f64) -> String {
    let millis = (value * 1000.0).round();
    if millis < 1000.0 {
        return format!("{millis:.0} ms");
    }
    if value < 60.0 {
        return format!("{value:.1} s");
    }
    let whole = value.round();
    let minutes = (whole / 60.0).floor();
    format!("{minutes:.0} min {:.0} s", whole - minutes * 60.0)
}

/// A time of `value` milliseconds, in the unit that reads best.
pub(super) fn millis(value: u64) -> String {
    seconds(Duration::from_millis(value).as_secs_f64())
}

pub(in crate::commands) fn latency(ms: f64) -> String {
    format!("{ms:.1} ms")
}

/// A share between 0 and 1, in percent.
pub(in crate::commands) fn percent(share: f64) -> String {
    format!("{:.2} %", share * 100.0)
}

pub(in crate::commands) fn rate(requests_per_s: f64) -> String {
    format!("{requests_per_s:.1} requests/s")
}

pub(super) fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

/// The time `ns` nanoseconds after the epoch, in UTC.
pub(super) fn utc(ns: i64) -> String {
    DateTime::from_timestamp_nanos(ns)
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string()
}

/// How long after the time `start_ns` the time `ns` is.
pub(super) fn offset(ns: i64, start_ns: i64) -> String {
    let since = ns.saturating_sub(start_ns);
    let sign = if since < 0 { "-" } else { "+" };
    let length = Duration::from_nanos(since.unsigned_abs());
    format!("{sign}{}", seconds(length.as_secs_f64()))
}

/// A word in the colour of `class`: `ok`, `warn`, `fail` or `muted`.
pub(super) fn badge(class: &str, word: &str) -> String {
    format!(r#"<span class="badge {class}">{}</span>"#, text(word))
}

/// A row of a table of facts: what the fact is, and its value as HTML.
pub(super) fn fact(label: &str, html: &str) -> String {
    format!("<tr><th>{label}</th><td>{html}</td></tr>\n")
}

/// The row of a fact the journal may not have.
pub(super) fn optional(label: &str, html: Option<String>) -> String {
    html.map_or_else(String::new, |html| fact(label, &html))
}

/// A table of facts from its rows. Nothing when there is no row.
pub(super) fn facts(rows: &str) -> String {
    if rows.is_empty() {
        return String::new();
    }
    format!("<table class=\"facts\">\n<tbody>\n{rows}</tbody>\n</table>\n")
}

/// A row of a table from its cells, which are HTML. A failed row is marked.
pub(super) fn tr(failed: bool, cells: &[String]) -> String {
    let open = if failed {
        r#"<tr class="row-failed">"#
    } else {
        "<tr>"
    };
    let cells: String = cells
        .iter()
        .map(|cell| ["<td>", cell, "</td>"].concat())
        .collect();
    format!("{open}{cells}</tr>\n")
}

/// A table from the names of its columns and its rows. Nothing when there
/// is no row.
pub(super) fn table(head: &[&str], rows: &str) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let head: String = head
        .iter()
        .map(|column| ["<th>", column, "</th>"].concat())
        .collect();
    format!("<table>\n<thead>\n<tr>{head}</tr>\n</thead>\n<tbody>\n{rows}</tbody>\n</table>\n")
}

/// A part of the report under its title. Nothing when it has no body.
pub(super) fn section(id: &str, title: &str, body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    format!("<section id=\"{id}\">\n<h2>{title}</h2>\n{body}</section>\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_written_without_the_decimals_that_say_nothing() {
        let written: Vec<String> = [30.0, 0.5, 13.3336, 100.0, 0.0, -0.0001, -2.5, 1200.0]
            .into_iter()
            .map(number)
            .collect();

        assert_eq!(
            written,
            ["30", "0.5", "13.334", "100", "0", "0", "-2.5", "1200"]
        );
    }

    #[test]
    fn a_time_is_written_in_the_unit_that_reads_best() {
        let written: Vec<String> = [
            0.0, 0.0004, 0.25, 0.9994, 0.9996, 1.0, 29.14, 59.9, 60.0, 89.6, 296.2, 3600.0,
        ]
        .into_iter()
        .map(seconds)
        .collect();

        assert_eq!(
            written,
            [
                "0 ms",
                "0 ms",
                "250 ms",
                "999 ms",
                "1.0 s",
                "1.0 s",
                "29.1 s",
                "59.9 s",
                "1 min 0 s",
                "1 min 30 s",
                "4 min 56 s",
                "60 min 0 s",
            ]
        );
    }

    #[test]
    fn milliseconds_are_written_as_the_time_they_are() {
        assert_eq!(millis(9), "9 ms");
        assert_eq!(millis(2024), "2.0 s");
        assert_eq!(millis(125_000), "2 min 5 s");
    }

    #[test]
    fn measures_carry_their_unit() {
        assert_eq!(latency(80.0), "80.0 ms");
        assert_eq!(latency(0.26), "0.3 ms");
        assert_eq!(percent(0.5), "50.00 %");
        assert_eq!(percent(0.000_12), "0.01 %");
        assert_eq!(rate(13.333), "13.3 requests/s");
        assert_eq!(yes_no(true), "yes");
        assert_eq!(yes_no(false), "no");
    }

    #[test]
    fn a_time_of_the_run_is_written_in_utc_and_as_an_offset_from_its_start() {
        let start = 1_794_867_319_686_756_002;

        assert_eq!(utc(start), "2026-11-16 22:15:19 UTC");
        assert_eq!(offset(start + 1_500_000_000, start), "+1.5 s");
        assert_eq!(offset(start + 12_000_000, start), "+12 ms");
        assert_eq!(offset(start, start), "+0 ms");
        assert_eq!(offset(start - 2_000_000_000, start), "-2.0 s");
    }

    #[test]
    fn a_table_without_rows_and_a_section_without_a_body_are_left_out() {
        assert_eq!(table(&["Probe"], ""), "");
        assert_eq!(facts(""), "");
        assert_eq!(section("load", "Load", ""), "");
        assert_eq!(optional("MTTR", None), "");
    }

    #[test]
    fn the_parts_of_a_table_are_the_html_of_a_table() {
        let rows = [
            tr(false, &["steady".to_string(), "ok".to_string()]),
            tr(true, &["broken".to_string(), String::new()]),
        ]
        .concat();

        assert_eq!(
            table(&["Probe", "Output"], &rows),
            "<table>\n<thead>\n<tr><th>Probe</th><th>Output</th></tr>\n</thead>\n<tbody>\n\
             <tr><td>steady</td><td>ok</td></tr>\n\
             <tr class=\"row-failed\"><td>broken</td><td></td></tr>\n\
             </tbody>\n</table>\n"
        );
        assert_eq!(
            facts(&optional("MTTR", Some("2.0 s".to_string()))),
            "<table class=\"facts\">\n<tbody>\n<tr><th>MTTR</th><td>2.0 s</td></tr>\n</tbody>\n</table>\n"
        );
        assert_eq!(
            section("load", "Load", "<p>none</p>\n"),
            "<section id=\"load\">\n<h2>Load</h2>\n<p>none</p>\n</section>\n"
        );
        assert_eq!(
            badge("ok", "a<b"),
            r#"<span class="badge ok">a&lt;b</span>"#
        );
    }
}

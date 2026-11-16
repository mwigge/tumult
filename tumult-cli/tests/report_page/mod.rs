//! The HTML report the binary writes, and how the tests read it.
//!
//! A report is written by `tumult report` to a file in the lab and read
//! from there: its sections by their id, its tables by their rows.
#![allow(dead_code)]

use std::path::Path;
use std::process::{Command, Output};

use crate::lab::Lab;

/// The time the reports of the tests are generated at, as the binary reads
/// it and as the report writes it.
pub const GENERATED: &str = "1794873600";
pub const GENERATED_TEXT: &str = "2026-11-17T00:00:00Z";

/// `tumult report` for `journal`, with the HTML written to the lab.
pub fn report_command(lab: &Lab, journal: &Path) -> Command {
    let journal = journal.to_string_lossy();
    let mut command = lab.command(&[
        "report",
        &journal,
        "--format",
        "html",
        "--output",
        "report.html",
    ]);
    command.env("SOURCE_DATE_EPOCH", GENERATED);
    command
}

/// The report `output` says was written.
pub fn written(lab: &Lab, output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(lab.path("report.html")).expect("report")
}

/// The HTML report the binary writes in `lab` for `journal`.
pub fn report_of(lab: &Lab, journal: &Path) -> String {
    let output = report_command(lab, journal).output().expect("run tumult");
    written(lab, &output)
}

/// The HTML report of the journal of the lab.
pub fn report(lab: &Lab) -> String {
    report_of(lab, &lab.path("journal.toon"))
}

/// The part of `html` after `start` and before `end`.
pub fn between<'a>(html: &'a str, start: &str, end: &str) -> &'a str {
    let Some((_, rest)) = html.split_once(start) else {
        panic!("no {start} in: {html}");
    };
    let Some((part, _)) = rest.split_once(end) else {
        panic!("no {end} after {start} in: {html}");
    };
    part
}

pub fn has_section(html: &str, id: &str) -> bool {
    html.contains(&format!(r#"<section id="{id}""#))
}

/// What is inside the section of the report whose id is `id`.
pub fn section<'a>(html: &'a str, id: &str) -> &'a str {
    let rest = between(html, &format!(r#"<section id="{id}""#), "</section>");
    rest.split_once('>').map_or(rest, |(_, inside)| inside)
}

/// `html` without its tags: its words, one space apart.
pub fn words(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// The rows of the tables in `html`: the words of each cell, header cells
/// included.
pub fn rows(html: &str) -> Vec<Vec<String>> {
    html.split("<tr")
        .skip(1)
        .map(|row| {
            // A row may say that it failed: `<tr class="row-failed">`.
            let row = row.split_once('>').map_or(row, |(_, cells)| cells);
            let row = row.split("</tr>").next().unwrap_or(row);
            let mut cells: Vec<String> = row
                .replace("</th>", "</td>")
                .split("</td>")
                .map(words)
                .collect();
            // What follows the last cell is no cell.
            cells.pop();
            cells
        })
        .collect()
}

/// The rows of `html` cut to the columns `keep`.
pub fn columns(html: &str, keep: &[usize]) -> Vec<Vec<String>> {
    rows(html)
        .into_iter()
        .map(|row| {
            keep.iter()
                .filter_map(|&column| row.get(column).cloned())
                .collect()
        })
        .collect()
}

pub fn row(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|cell| (*cell).to_string()).collect()
}

/// The rows of `html` that are marked as failed, cut to their first cell.
pub fn failed_rows(html: &str) -> Vec<String> {
    html.split(r#"<tr class="row-failed">"#)
        .skip(1)
        .map(|row| words(row.split("</td>").next().unwrap_or(row)))
        .collect()
}

/// The labels of the summary cards, in their order.
pub fn card_labels(html: &str) -> Vec<String> {
    section(html, "summary")
        .split(r#"<div class="label">"#)
        .skip(1)
        .map(|card| words(card.split("</div>").next().unwrap_or(card)))
        .collect()
}

/// The value of the summary card `label`.
pub fn card(html: &str, label: &str) -> String {
    let cards = section(html, "summary");
    let start = format!(r#"<div class="label">{label}</div>"#);
    words(between(cards, &start, "</div>"))
}

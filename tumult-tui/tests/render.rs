//! Headless render + interaction tests against a seeded on-disk store.
//!
//! These drive the real `App` (which opens the `DuckDB` store read-only) and
//! render into ratatui's `TestBackend`, so the full query → model → UI path is
//! exercised without a terminal. They give the TUI repeatable, CI-friendly
//! proof that it works against the actual analytics schema.

// Test fixtures build timestamps from small positive literals; the u64→i64
// nanosecond casts cannot wrap for any value used here.
#![allow(clippy::cast_possible_wrap)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use tempfile::TempDir;

use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, ExperimentStatus, Journal,
};
use tumult_lake::{AnalyticsStore, RunVerdict};
use tumult_tui::app::{App, Tab, View};
use tumult_tui::logic::StatusKind;
use tumult_tui::{data, ui};

fn journal(id: &str, title: &str, status: ExperimentStatus, start_ns: i64, dur_ms: u64) -> Journal {
    let failed = status != ExperimentStatus::Completed;
    Journal {
        target_version: None,
        target: None,
        experiment_title: title.into(),
        experiment_id: id.into(),
        status,
        started_at_ns: start_ns,
        ended_at_ns: start_ns + (dur_ms as i64) * 1_000_000,
        duration_ms: dur_ms,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![
            ActivityResult {
                name: "probe-health".into(),
                activity_type: ActivityType::Probe,
                status: ActivityStatus::Succeeded,
                started_at_ns: start_ns + 1,
                duration_ms: 40,
                output: Some("ok".into()),
                error: None,
                trace_id: "t".into(),
                span_id: "s".into(),
            },
            ActivityResult {
                name: "inject-latency".into(),
                activity_type: ActivityType::Action,
                status: if failed {
                    ActivityStatus::Failed
                } else {
                    ActivityStatus::Succeeded
                },
                started_at_ns: start_ns + 2,
                duration_ms: dur_ms,
                output: Some("applied".into()),
                error: failed.then(|| "boom".into()),
                trace_id: "t".into(),
                span_id: "s".into(),
            },
        ],
        rollback_results: vec![],
        rollback_failures: 0,
        kind: tumult_core::types::TestKind::default(),
        objectives: None,
        probe_samples: vec![],
        halt: None,
        blast_radius: None,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: Some(AnalysisResult {
            estimate_accuracy: Some(1.0),
            estimate_recovery_delta_s: None,
            trend: None,
            resilience_score: Some(if failed { 0.6 } else { 0.95 }),
        }),
        regulatory: None,
    }
}

fn seed() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("analytics.duckdb");
    let store = AnalyticsStore::open(&path).unwrap();
    let base = 1_700_000_000_000_000_000i64;
    store
        .ingest_journal(&journal(
            "e1",
            "Latency drill",
            ExperimentStatus::Completed,
            base,
            120,
        ))
        .unwrap();
    store
        .ingest_journal(&journal(
            "e2",
            "Packet loss storm",
            ExperimentStatus::Deviated,
            base + 60_000_000_000,
            8000,
        ))
        .unwrap();
    store
        .ingest_journal(&journal(
            "e3",
            "CPU saturation",
            ExperimentStatus::Completed,
            base + 120_000_000_000,
            3000,
        ))
        .unwrap();
    drop(store);
    (dir, path)
}

/// The names the deserializer of a status expects, read from the error it
/// gives for a name it does not know.
fn names_expected(refused: &serde_json::Error) -> Vec<serde_json::Value> {
    let refused = refused.to_string();
    let names = refused
        .split_once("expected one of ")
        .and_then(|(_, rest)| rest.split_once(" at line"))
        .map_or_else(
            || panic!("the refusal does not list the statuses: {refused}"),
            |(names, _)| names,
        );
    names
        .split(", ")
        .map(|name| serde_json::json!(name.trim_matches('`')))
        .collect()
}

/// Every status a run can end with, so that a status added to
/// `ExperimentStatus` is in the list.
fn every_run_status() -> Vec<ExperimentStatus> {
    let refused =
        serde_json::from_str::<ExperimentStatus>("\"none\"").expect_err("no run ends as none");
    names_expected(&refused)
        .into_iter()
        .map(|name| serde_json::from_value(name).unwrap())
        .collect()
}

/// Every status a step can end with, so that a status added to
/// `ActivityStatus` is in the list.
fn every_step_status() -> Vec<ActivityStatus> {
    let refused =
        serde_json::from_str::<ActivityStatus>("\"none\"").expect_err("no step ends as none");
    names_expected(&refused)
        .into_iter()
        .map(|name| serde_json::from_value(name).unwrap())
        .collect()
}

/// A lake with one run for each of `statuses`, a minute apart. Each run has
/// one step for each status a step can end with.
fn seed_every_status(statuses: &[ExperimentStatus]) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("analytics.duckdb");
    let store = AnalyticsStore::open(&path).unwrap();
    let base = 1_700_000_000_000_000_000i64;
    for (minute, status) in (0i64..).zip(statuses) {
        let mut run = journal(
            &format!("run-{status}"),
            &format!("Run that ended {status}"),
            status.clone(),
            base + minute * 60_000_000_000,
            1000,
        );
        let step = run.method_results[0].clone();
        run.method_results = every_step_status()
            .into_iter()
            .map(|status| ActivityResult {
                name: format!("step-{status}"),
                status,
                ..step.clone()
            })
            .collect();
        store.ingest_journal(&run).unwrap();
    }
    drop(store);
    (dir, path)
}

/// The colour of the first cell of `word` on the line of the screen that
/// shows `line_with`; `None` when no such line shows `word`.
fn colour_on_line(term: &Terminal<TestBackend>, line_with: &str, word: &str) -> Option<Color> {
    let buffer = term.backend().buffer();
    (0..buffer.area.height).find_map(|y| {
        let cells: Vec<&ratatui::buffer::Cell> =
            (0..buffer.area.width).map(|x| &buffer[(x, y)]).collect();
        let symbols: Vec<&str> = cells.iter().map(|cell| cell.symbol()).collect();
        if !symbols.concat().contains(line_with) {
            return None;
        }
        (0..symbols.len())
            .find(|at| {
                symbols[*at..]
                    .iter()
                    .take(word.chars().count())
                    .copied()
                    .collect::<String>()
                    == word
            })
            .map(|at| cells[at].fg)
    })
}

fn buffer_text(term: &Terminal<TestBackend>) -> String {
    term.backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn experiments_tab_renders_history_and_pills() {
    let (_dir, path) = seed();
    let mut app = App::new(path, 2).unwrap();
    assert_eq!(app.experiment_count, 3);
    assert_eq!(app.filtered.len(), 3);

    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();
    let text = buffer_text(&term);

    // Tab strip + all four tabs present.
    for t in Tab::ALL {
        assert!(text.contains(t.title()), "missing tab {}", t.title());
    }
    // History rows, newest first (CPU saturation started last).
    assert!(text.contains("Latency drill"));
    assert!(text.contains("Packet loss storm"));
    assert!(text.contains("CPU saturation"));
    // Status pills.
    assert!(text.contains("PASS"));
    assert!(text.contains("DEVIATED"));
    // Header shows the live indicator and counts.
    assert!(text.contains("LIVE"));
    assert!(text.contains("3 experiments"));
}

#[test]
fn newest_experiment_is_first_row() {
    let (_dir, path) = seed();
    let app = App::new(path, 2).unwrap();
    // Default sort is time-descending.
    assert_eq!(app.selected_experiment().unwrap().title, "CPU saturation");
}

#[test]
fn title_filter_narrows_and_clears() {
    let (_dir, path) = seed();
    let mut app = App::new(path, 2).unwrap();
    app.on_key(key(KeyCode::Char('/')));
    for c in "packet".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.filtered.len(), 1);
    assert_eq!(
        app.selected_experiment().unwrap().title,
        "Packet loss storm"
    );
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.filtered.len(), 3);
}

#[test]
fn drill_in_shows_activity_timeline() {
    let (_dir, path) = seed();
    let mut app = App::new(path, 2).unwrap();
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.view, View::Detail);

    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("Timeline"));
    assert!(text.contains("inject-latency"));
    assert!(text.contains("probe-health"));
}

#[test]
fn analytics_tab_renders_trends() {
    let (_dir, path) = seed();
    let mut app = App::new(path, 2).unwrap();
    app.on_key(key(KeyCode::Char('2')));
    assert_eq!(app.tab, Tab::Analytics);

    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("success rate"));
    assert!(text.contains("Status breakdown"));
    assert!(text.contains("Trends"));
}

#[test]
fn chaosgraph_tab_lists_nodes() {
    let (_dir, path) = seed();
    let mut app = App::new(path, 2).unwrap();
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(app.tab, Tab::ChaosGraph);
    // The seeded runs contribute experiment nodes to the graph.
    assert!(!app.graph_nodes.is_empty());

    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("ChaosGraph"));
    assert!(text.contains("experiment"));
}

#[test]
fn compare_view_renders_marked_runs() {
    let (_dir, path) = seed();
    let mut app = App::new(path, 2).unwrap();
    app.on_key(key(KeyCode::Char(' '))); // mark newest
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char(' '))); // mark second
    app.on_key(key(KeyCode::Char('c')));
    assert_eq!(app.view, View::Compare);

    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("Compare"));
    assert!(text.contains("Duration trend"));
}

#[test]
fn every_status_a_run_ends_with_has_its_own_pill_in_the_tone_of_its_verdict() {
    let statuses = every_run_status();
    let (_dir, path) = seed_every_status(&statuses);
    let mut app = App::new(path, 2).unwrap();
    // The selected row is drawn in the colours of the selection.
    app.table_state.select(None);

    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();

    let mut drawn = Vec::new();
    let mut wanted = Vec::new();
    for status in &statuses {
        let pill = match status {
            ExperimentStatus::Completed => "PASS".to_string(),
            ExperimentStatus::Failed => "FAIL".to_string(),
            other => other.to_string().to_ascii_uppercase(),
        };
        // A run that passed is green, a run that failed is red, and a run
        // that proves nothing either way is grey.
        let tone = match tumult_lake::run_verdict(&status.to_string()) {
            Some(RunVerdict::Pass) => Color::Green,
            Some(RunVerdict::Fail) => Color::Red,
            Some(RunVerdict::NoEvidence) => Color::Gray,
            None => panic!("{status} has no verdict"),
        };
        let title = format!("Run that ended {status}");
        drawn.push((status, colour_on_line(&term, &title, &pill)));
        wanted.push((status, Some(tone)));
    }
    assert_eq!(drawn, wanted);
    assert!(!buffer_text(&term).contains("UNKNOWN"));
}

#[test]
fn the_compare_view_shows_the_whole_pill_of_every_marked_run() {
    let statuses = every_run_status();
    let (_dir, path) = seed_every_status(&statuses);
    let mut app = App::new(path, 2).unwrap();
    for _ in &statuses {
        app.on_key(key(KeyCode::Char(' ')));
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Char('c')));
    assert_eq!(app.view, View::Compare);
    assert_eq!(app.marked_experiments().len(), statuses.len());

    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();

    // The longest pill has eleven letters: the column must not cut it.
    assert_eq!(
        colour_on_line(&term, "Run that ended interrupted", "INTERRUPTED"),
        Some(Color::Gray),
        "{}",
        buffer_text(&term)
    );
}

#[test]
fn the_timeline_draws_each_step_in_the_tone_of_how_it_ended() {
    let (_dir, path) = seed_every_status(&[ExperimentStatus::Completed]);
    let mut app = App::new(path, 2).unwrap();
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.view, View::Detail);

    let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();
    term.draw(|f| ui::render(&mut app, f)).unwrap();

    // The bar of a step is the first thing on its line.
    let bars: Vec<(String, Option<Color>)> = every_step_status()
        .iter()
        .map(|status| {
            let step = format!("step-{status}");
            let bar = colour_on_line(&term, &step, "\u{2588}");
            (step, bar)
        })
        .collect();
    assert_eq!(
        bars,
        [
            ("step-succeeded".to_string(), Some(Color::Green)),
            ("step-failed".to_string(), Some(Color::Red)),
            ("step-timeout".to_string(), Some(Color::Red)),
            ("step-skipped".to_string(), Some(Color::Gray)),
        ],
        "{}",
        buffer_text(&term)
    );
}

#[test]
fn every_status_a_step_ends_with_is_known_to_the_timeline() {
    let (_dir, path) = seed_every_status(&[ExperimentStatus::Completed]);

    let steps = data::load_activities(&path, "run-completed").unwrap();

    assert_eq!(steps.len(), every_step_status().len());
    for step in &steps {
        assert_ne!(
            StatusKind::of_step(&step.status),
            StatusKind::OTHER,
            "{}",
            step.status
        );
    }
}

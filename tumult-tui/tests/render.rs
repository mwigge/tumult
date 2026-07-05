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
use ratatui::Terminal;
use tempfile::TempDir;

use tumult_analytics::AnalyticsStore;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, ExperimentStatus, Journal,
};
use tumult_tui::app::{App, Tab, View};
use tumult_tui::ui;

fn journal(id: &str, title: &str, status: ExperimentStatus, start_ns: i64, dur_ms: u64) -> Journal {
    let failed = status != ExperimentStatus::Completed;
    Journal {
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

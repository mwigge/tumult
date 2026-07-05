//! `tumult-tui` — a keyboard-driven, tabbed analytics TUI over the embedded
//! `DuckDB` analytics store.
//!
//! The store is always opened **read-only** ([`tumult_analytics::AnalyticsStore::open_read_only`]),
//! so the TUI coexists with a running MCP server or a concurrent `tumult run`
//! ingest without contending for the exclusive write lock. In live mode it
//! re-takes a fresh read-only snapshot every refresh interval, so
//! newly-completed experiments appear in near-real time.
//!
//! The public entry point is [`run`], wired into the CLI as `tumult tui`.
//!
//! # Layout
//!
//! * [`model`] — typed rows mapped from stringly-typed query results.
//! * [`logic`] — pure, unit-tested sort/filter/format/trend/micro-bar helpers.
//! * [`data`] — read-only store access returning typed rows.
//! * [`app`] — the tab/selection/filter/live state machine and key handling.
//! * [`ui`] — ratatui rendering (a projection over `app` + `logic`).

// A terminal UI does a great deal of `usize`/`u64`/`f64` layout and ratio
// arithmetic where the lossy-cast pedantic lints add noise without catching
// real bugs; the values are all small, bounded row/column counts and durations.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::module_name_repetitions,
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::must_use_candidate
)]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use crossterm::event::{self, Event, KeyEventKind};

pub mod app;
pub mod data;
pub mod logic;
pub mod model;
pub mod theme;
pub mod ui;

use app::App;

/// How long to block for input before checking whether a live refresh is due.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Launch the analytics TUI.
///
/// `store_path` overrides the default `~/.tumult/analytics.duckdb`; `refresh_secs`
/// is the live-mode poll interval (clamped to at least 1 second).
///
/// # Errors
///
/// Returns an error if the store does not exist, cannot be opened read-only,
/// or the terminal cannot be driven.
pub fn run(store_path: Option<PathBuf>, refresh_secs: u64) -> Result<()> {
    let path = store_path.unwrap_or_else(tumult_analytics::AnalyticsStore::default_path);
    if !path.exists() {
        bail!(
            "no analytics store at {}\n\
             Run an experiment first (e.g. `tumult run experiment.toon`) to create it, \
             or pass --store <path>.",
            path.display()
        );
    }

    let mut app = App::new(path, refresh_secs)?;

    // `ratatui::init` enters the alternate screen, enables raw mode, and
    // installs a panic hook that restores the terminal before unwinding.
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

/// The draw/input/refresh loop. Returns `Ok(())` on a clean quit.
fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(app, f))?;

        if event::poll(POLL_INTERVAL)? {
            if let Event::Key(key) = event::read()? {
                // Ignore key-release/repeat events on platforms that report them.
                if key.kind == KeyEventKind::Press && app.on_key(key) {
                    return Ok(());
                }
            }
        }
        app.tick();
    }
}

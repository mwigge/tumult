//! Colour theme and status-pill styling. Kept small and centralised so the
//! whole UI reads as one palette.

use ratatui::style::{Color, Modifier, Style};

use crate::logic::StatusKind;

/// Accent used for the active tab, selection, and headings.
pub const ACCENT: Color = Color::Cyan;
/// Muted colour for secondary text and inactive chrome.
pub const MUTED: Color = Color::DarkGray;
/// Highlight background for a freshly-arrived (new since last refresh) row.
pub const NEW_ROW_BG: Color = Color::Rgb(40, 60, 40);

/// The colour associated with a status outcome.
#[must_use]
pub fn status_color(kind: StatusKind) -> Color {
    match kind {
        StatusKind::Pass => Color::Green,
        StatusKind::Deviated => Color::Yellow,
        StatusKind::Failed => Color::Red,
        StatusKind::Aborted => Color::Magenta,
        StatusKind::Running => Color::Cyan,
        StatusKind::Other => Color::Gray,
    }
}

/// A bold, coloured style for a status pill.
#[must_use]
pub fn status_style(kind: StatusKind) -> Style {
    Style::default()
        .fg(status_color(kind))
        .add_modifier(Modifier::BOLD)
}

/// Style for the selected table row.
#[must_use]
pub fn selection_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

/// Style for the live indicator: green when live, muted when paused.
#[must_use]
pub fn live_style(live: bool) -> Style {
    if live {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    }
}

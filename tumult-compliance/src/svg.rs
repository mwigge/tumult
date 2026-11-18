//! Minimal hand-rolled SVG charts (bars, donut, trend, sparkline) in the Okabe-Ito
//! colorblind-safe palette. Kept deliberately small: direct labels, no
//! gridline machinery.

use crate::model::ChartSpec;

/// Okabe-Ito palette (colorblind-safe), black first for neutral series.
pub const PALETTE: [&str; 8] = [
    "#0072B2", "#E69F00", "#009E73", "#D55E00", "#56B4E9", "#CC79A7", "#F0E442", "#000000",
];

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const FONT: &str = "font-family='Inter, sans-serif'";

/// Render a chart spec to a standalone SVG string.
#[must_use]
pub fn render_svg(spec: &ChartSpec) -> String {
    match spec {
        ChartSpec::Bars(rows) => bars(rows),
        ChartSpec::Donut(slices) => donut(slices),
        ChartSpec::Trend { y_name, series } => trend(y_name, series),
        ChartSpec::Sparkline(points) => sparkline(points),
    }
}

/// Horizontal bars in caller order, with direct value labels at bar ends.
/// Every bar has the first colour of the palette: a bar is a number, so it
/// takes no colour a status cell judges with.
fn bars(rows: &[(String, f64)]) -> String {
    let label_w = 170.0_f64;
    let bar_max_w = 240.0_f64;
    let row_h = 22.0_f64;
    let max = rows.iter().map(|r| r.1).fold(0.0_f64, f64::max).max(1e-9);
    let h = rows.len() as f64 * row_h + 8.0;
    let mut out = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='480' height='{h:.0}' viewBox='0 0 480 {h:.0}'>"
    );
    let color = PALETTE[0];
    for (i, (label, v)) in rows.iter().enumerate() {
        let y = i as f64 * row_h + 4.0;
        let w = (v / max * bar_max_w).max(1.0);
        out.push_str(&format!(
            "<text x='0' y='{ty:.1}' font-size='9.5' fill='#333' {FONT}>{label}</text>\
             <rect x='{label_w}' y='{y:.1}' width='{w:.1}' height='14' fill='{color}'/>\
             <text x='{tx:.1}' y='{ty:.1}' font-size='9.5' fill='#333' {FONT}>{v}</text>",
            ty = y + 11.0,
            tx = label_w + w + 6.0,
            label = esc(&truncate(label, 30)),
            v = fmt_num(*v),
        ));
    }
    out.push_str("</svg>");
    out
}

/// Ellipsize a label to `max` chars (char-boundary safe).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Donut with a legend; shares as glyph + label + percentage.
fn donut(slices: &[(String, f64)]) -> String {
    let total: f64 = slices.iter().map(|s| s.1).sum();
    if total <= 0.0 {
        return String::new();
    }
    let (cx, cy, r, ir) = (70.0_f64, 70.0, 56.0, 30.0);
    let mut out = String::from(
        "<svg xmlns='http://www.w3.org/2000/svg' width='400' height='140' viewBox='0 0 400 140'>",
    );
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (i, (label, v)) in slices.iter().enumerate() {
        let frac = v / total;
        let sweep = frac * std::f64::consts::TAU;
        let color = PALETTE[i % PALETTE.len()];
        if frac > 0.999_999 {
            // Full circle: two half arcs.
            out.push_str(&format!(
                "<circle cx='{cx}' cy='{cy}' r='{r}' fill='{color}'/>\
                 <circle cx='{cx}' cy='{cy}' r='{ir}' fill='#fff'/>"
            ));
        } else if frac > 0.0 {
            let a1 = angle;
            let a2 = angle + sweep;
            let large = usize::from(sweep > std::f64::consts::PI);
            let p = |a: f64, rad: f64| (cx + rad * a.cos(), cy + rad * a.sin());
            let (x1, y1) = p(a1, r);
            let (x2, y2) = p(a2, r);
            let (x3, y3) = p(a2, ir);
            let (x4, y4) = p(a1, ir);
            out.push_str(&format!(
                "<path d='M{x1:.2} {y1:.2} A{r} {r} 0 {large} 1 {x2:.2} {y2:.2} \
                 L{x3:.2} {y3:.2} A{ir} {ir} 0 {large} 0 {x4:.2} {y4:.2} Z' fill='{color}'/>"
            ));
        }
        let ly = 20.0 + i as f64 * 18.0;
        out.push_str(&format!(
            "<rect x='150' y='{ly:.1}' width='10' height='10' fill='{color}'/>\
             <text x='166' y='{ty:.1}' font-size='10' fill='#333' {FONT}>{label} — {pct}%</text>",
            ly = ly - 9.0,
            ty = ly,
            label = esc(label),
            pct = fmt_num(frac * 100.0),
        ));
        angle += sweep;
    }
    out.push_str("</svg>");
    out
}

/// Where the plot of a trend is on its canvas, and what it shows.
struct TrendFrame {
    /// The time of the first and of the last point.
    x: (i64, i64),
    /// The value at the bottom and at the top of the plot.
    y: (f64, f64),
}

impl TrendFrame {
    const WIDTH: f64 = 480.0;
    const HEIGHT: f64 = 180.0;
    const LEFT: f64 = 52.0;
    const RIGHT: f64 = 90.0;
    const TOP: f64 = 10.0;
    const BOTTOM: f64 = 40.0;
    /// The right end of the x axis.
    const END: f64 = Self::WIDTH - Self::RIGHT;
    /// The height of the x axis on the canvas.
    const FLOOR: f64 = Self::HEIGHT - Self::BOTTOM;

    /// The frame that holds every point of `series`; `None` without a point.
    fn around(series: &[(String, Vec<(i64, f64)>)]) -> Option<Self> {
        let points = || series.iter().flat_map(|s| s.1.iter());
        let first = points().map(|p| p.0).min()?;
        let last = points().map(|p| p.0).max()?;
        let low = points().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let high = points().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        // Pad the range so the trend shape is readable (no forced zero
        // baseline — that flattens any score trend into a straight line).
        // Non-negative series (scores, counts) never dip below zero though.
        let pad = ((high - low) * 0.15).max(2.0);
        let bottom = if low >= 0.0 {
            (low - pad).max(0.0)
        } else {
            low - pad
        };
        Some(Self {
            x: (first, last),
            y: (bottom, high + pad),
        })
    }

    /// Where the time `x` is on the canvas.
    fn sx(&self, x: i64) -> f64 {
        let span = ((self.x.1 - self.x.0) as f64).max(1e-9);
        Self::LEFT + (x - self.x.0) as f64 / span * (Self::END - Self::LEFT)
    }

    /// Where the value `y` is on the canvas.
    fn sy(&self, y: f64) -> f64 {
        let share = (y - self.y.0) / (self.y.1 - self.y.0);
        Self::TOP + (1.0 - share) * (Self::FLOOR - Self::TOP)
    }
}

/// The instants of `series` that get their date on the x axis: the first,
/// the middle and the last one.
fn dated_instants(series: &[(String, Vec<(i64, f64)>)]) -> Vec<i64> {
    let mut instants: Vec<i64> = series
        .iter()
        .flat_map(|s| s.1.iter().map(|p| p.0))
        .collect();
    instants.sort_unstable();
    instants.dedup();
    let Some(last) = instants.len().checked_sub(1) else {
        return Vec::new();
    };
    let mut dated = vec![instants[0], instants[last / 2 + last % 2], instants[last]];
    dated.dedup();
    dated
}

/// Both axes of a trend: the lines, the lowest and the highest value, the
/// name of the y axis, the dates and the name of the x axis.
fn trend_axes(frame: &TrendFrame, y_name: &str, dated: &[i64]) -> String {
    let (left, top, floor) = (TrendFrame::LEFT, TrendFrame::TOP, TrendFrame::FLOOR);
    let mut out = format!(
        "<line x1='{left}' y1='{floor}' x2='{right}' y2='{floor}' stroke='#999' stroke-width='0.7'/>\
         <line x1='{left}' y1='{top}' x2='{left}' y2='{floor}' stroke='#999' stroke-width='0.7'/>\
         <text x='{tick_x}' y='{high_y:.1}' text-anchor='end' font-size='8' fill='#666' {FONT}>{high}</text>\
         <text x='{tick_x}' y='{floor:.1}' text-anchor='end' font-size='8' fill='#666' {FONT}>{low}</text>\
         <text x='10' y='{mid_y:.1}' text-anchor='middle' transform='rotate(-90 10 {mid_y:.1})' font-size='8.5' fill='#333' {FONT}>{y_name}</text>\
         <text x='{mid_x:.1}' y='{name_y:.1}' text-anchor='middle' font-size='8.5' fill='#333' {FONT}>Date (UTC)</text>",
        right = TrendFrame::END,
        tick_x = left - 4.0,
        high_y = top + 7.0,
        high = fmt_num(frame.y.1),
        low = fmt_num(frame.y.0),
        mid_y = (top + floor) / 2.0,
        mid_x = (left + TrendFrame::END) / 2.0,
        name_y = TrendFrame::HEIGHT - 6.0,
        y_name = esc(y_name),
    );
    let mut shown = String::new();
    for (i, instant) in dated.iter().enumerate() {
        let date = crate::html::fmt_date(*instant);
        // A trend inside one day has one date, not the same one three times.
        if date == shown {
            continue;
        }
        let anchor = match i {
            0 => "start",
            _ if i + 1 == dated.len() => "end",
            _ => "middle",
        };
        out.push_str(&format!(
            "<line x1='{x:.1}' y1='{floor}' x2='{x:.1}' y2='{tick}' stroke='#999' stroke-width='0.7'/>\
             <text x='{x:.1}' y='{y:.1}' text-anchor='{anchor}' font-size='8' fill='#666' {FONT}>{date}</text>",
            x = frame.sx(*instant),
            tick = floor + 3.0,
            y = floor + 13.0,
        ));
        shown = date;
    }
    out
}

/// One series of a trend: its line, a marker on each point and its name at
/// the end of the line.
fn trend_series(frame: &TrendFrame, name: &str, points: &[(i64, f64)], color: &str) -> String {
    let Some(last) = points.last() else {
        return String::new();
    };
    let path: Vec<String> = points
        .iter()
        .map(|p| format!("{:.1},{:.1}", frame.sx(p.0), frame.sy(p.1)))
        .collect();
    let mut out = format!(
        "<polyline points='{}' fill='none' stroke='{color}' stroke-width='1.6'/>",
        path.join(" ")
    );
    // Always draw point markers so single-point series are visible.
    for p in points {
        out.push_str(&format!(
            "<circle cx='{:.1}' cy='{:.1}' r='2.2' fill='{color}'/>",
            frame.sx(p.0),
            frame.sy(p.1)
        ));
    }
    out.push_str(&format!(
        "<text x='{:.1}' y='{:.1}' font-size='9' fill='{color}' {FONT}>{}</text>",
        frame.sx(last.0) + 5.0,
        frame.sy(last.1) + 3.0,
        esc(name)
    ));
    out
}

/// Lines over time with the name of each series at the end of its line,
/// named axes and dates on the x axis.
fn trend(y_name: &str, series: &[(String, Vec<(i64, f64)>)]) -> String {
    let Some(frame) = TrendFrame::around(series) else {
        return String::new();
    };
    let mut out = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w:.0}' height='{h:.0}' viewBox='0 0 {w:.0} {h:.0}'>",
        w = TrendFrame::WIDTH,
        h = TrendFrame::HEIGHT,
    );
    out.push_str(&trend_axes(&frame, y_name, &dated_instants(series)));
    for (i, (name, points)) in series.iter().enumerate() {
        out.push_str(&trend_series(
            &frame,
            name,
            points,
            PALETTE[i % PALETTE.len()],
        ));
    }
    out.push_str("</svg>");
    out
}

/// A small line of values over time with a marker on the last one. It has
/// no axes and no text: the card it sits in says what it shows.
fn sparkline(points: &[(i64, f64)]) -> String {
    const WIDTH: f64 = 120.0;
    const HEIGHT: f64 = 28.0;
    const PAD: f64 = 3.0;
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return String::new();
    };
    let low = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let high = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    // One point has no time to run over: it sits at the end, where the
    // last point of a line is.
    let sx = |t: i64| {
        if last.0 == first.0 {
            return WIDTH - PAD;
        }
        PAD + (t - first.0) as f64 / (last.0 - first.0) as f64 * (WIDTH - 2.0 * PAD)
    };
    // Equal values have no range to fill: they sit in the middle.
    let sy = |v: f64| {
        if high == low {
            return HEIGHT / 2.0;
        }
        HEIGHT - PAD - (v - low) / (high - low) * (HEIGHT - 2.0 * PAD)
    };
    let color = PALETTE[0];
    let mut out = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{WIDTH:.0}' height='{HEIGHT:.0}' viewBox='0 0 {WIDTH:.0} {HEIGHT:.0}'>"
    );
    if points.len() > 1 {
        let path: Vec<String> = points
            .iter()
            .map(|p| format!("{:.1},{:.1}", sx(p.0), sy(p.1)))
            .collect();
        out.push_str(&format!(
            "<polyline points='{}' fill='none' stroke='{color}' stroke-width='1.4'/>",
            path.join(" ")
        ));
    }
    out.push_str(&format!(
        "<circle cx='{:.1}' cy='{:.1}' r='2' fill='{color}'/></svg>",
        sx(last.0),
        sy(last.1)
    ));
    out
}

/// Trim trailing zeros for compact chart labels.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.1}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000_000_000;

    /// A trend of scores with the series `series`.
    fn trend_of(series: Vec<(String, Vec<(i64, f64)>)>) -> ChartSpec {
        ChartSpec::Trend {
            y_name: "Score".into(),
            series,
        }
    }

    #[test]
    fn bars_render_labels_and_rects() {
        let svg = render_svg(&ChartSpec::Bars(vec![
            ("pg".into(), 100.0),
            ("redis".into(), 50.0),
        ]));
        assert!(svg.contains("<rect"));
        assert!(svg.contains("pg"));
        assert!(svg.contains("100"));
    }

    #[test]
    fn every_bar_is_drawn_in_one_colour_whatever_its_place() {
        // A bar shows a number, not a judgement: its place in the list must
        // not give it the colour a status cell uses for good, warn or bad.
        let svg = render_svg(&ChartSpec::Bars(
            (0..PALETTE.len())
                .map(|number| (format!("experiment {number}"), 100.0))
                .collect(),
        ));

        let fills: Vec<&str> = svg
            .split("<rect")
            .skip(1)
            .map(|bar| {
                let fill = bar.split("fill='").nth(1).expect("a fill");
                &fill[..fill.find('\'').expect("the end of the fill")]
            })
            .collect();
        assert_eq!(fills, [PALETTE[0]; PALETTE.len()], "{svg}");
    }

    #[test]
    fn donut_handles_full_circle_and_empty() {
        let full = render_svg(&ChartSpec::Donut(vec![("passed".into(), 6.0)]));
        assert!(full.contains("<circle"));
        assert!(render_svg(&ChartSpec::Donut(vec![])).is_empty());
    }

    #[test]
    fn single_point_line_still_draws_a_marker() {
        let svg = render_svg(&trend_of(vec![("score".into(), vec![(DAY, 87.0)])]));
        assert!(svg.contains("<circle"));
        assert!(svg.contains("score"));
    }

    #[test]
    fn long_bar_labels_are_truncated() {
        let svg = render_svg(&ChartSpec::Bars(vec![(
            "api-worker freeze — heartbeat recovers after SIGSTOP injection".into(),
            100.0,
        )]));
        assert!(svg.contains('…'), "{svg}");
        assert!(!svg.contains("SIGSTOP injection"));
    }

    #[test]
    fn donut_draws_arcs_for_partial_slices_and_escapes_labels() {
        let svg = render_svg(&ChartSpec::Donut(vec![
            ("passed".into(), 3.0),
            ("failed <script>".into(), 1.0),
            ("zero".into(), 0.0),
        ]));
        // Two non-zero slices: two arc paths, and a legend entry each
        // (including the zero slice).
        assert_eq!(svg.matches("<path").count(), 2, "{svg}");
        assert!(svg.contains("passed — 75%"), "{svg}");
        assert!(svg.contains("failed &lt;script&gt; — 25%"), "{svg}");
        assert!(svg.contains("zero — 0%"), "{svg}");
    }

    #[test]
    fn lines_skips_empty_series_and_pads_negative_ranges() {
        // An empty point list contributes nothing but must not blank the
        // chart; negative values pad below zero.
        let svg = render_svg(&trend_of(vec![
            ("empty".into(), vec![]),
            ("delta".into(), vec![(DAY, -4.0), (2 * DAY, 6.5)]),
        ]));
        assert!(svg.contains("<polyline"), "{svg}");
        assert!(svg.contains("delta"), "{svg}");
        assert!(!svg.contains("empty</text>"), "{svg}");
        // Axis labels: y1 = 6.5 + pad → 8.5; y0 = -4 - pad → -6.
        assert!(svg.contains(">8.5<"), "{svg}");
        assert!(svg.contains(">-6<"), "{svg}");
    }

    #[test]
    fn lines_with_no_points_at_all_render_nothing() {
        assert!(render_svg(&trend_of(vec![])).is_empty());
        assert!(render_svg(&trend_of(vec![("a".into(), vec![])])).is_empty());
    }

    /// 2026-11-01 00:00 UTC.
    const NOV_1: i64 = 20_758 * DAY;

    /// The `x` of the text `label` in `svg`.
    fn x_of_text(svg: &str, label: &str) -> String {
        let end = svg
            .find(&format!(">{label}</text>"))
            .unwrap_or_else(|| panic!("no text {label} in {svg}"));
        let element = &svg[svg[..end].rfind("<text").expect("a text element")..end];
        let x = element.split("x='").nth(1).expect("an x");
        x[..x.find('\'').expect("the end of x")].to_string()
    }

    /// The `cx` of each point marker in `svg`.
    fn marker_xs(svg: &str) -> Vec<String> {
        svg.split("<circle cx='")
            .skip(1)
            .map(|rest| rest[..rest.find('\'').expect("the end of cx")].to_string())
            .collect()
    }

    /// A weekly trend of five points that starts on the first of November.
    fn five_weeks() -> ChartSpec {
        let points = (0..5).map(|week| (NOV_1 + 7 * week * DAY, 60.0 + week as f64));
        trend_of(vec![("portfolio".into(), points.collect())])
    }

    #[test]
    fn a_trend_names_what_its_axes_measure() {
        let svg = render_svg(&five_weeks());

        assert_eq!(svg.matches(">Score</text>").count(), 1, "{svg}");
        assert_eq!(svg.matches(">Date (UTC)</text>").count(), 1, "{svg}");
    }

    #[test]
    fn a_trend_shows_the_date_of_its_first_its_middle_and_its_last_point() {
        let svg = render_svg(&five_weeks());
        let markers = marker_xs(&svg);

        assert_eq!(markers.len(), 5, "{svg}");
        assert_eq!(x_of_text(&svg, "2026-11-01"), markers[0]);
        assert_eq!(x_of_text(&svg, "2026-11-15"), markers[2]);
        assert_eq!(x_of_text(&svg, "2026-11-29"), markers[4]);
        // The points between them have no date of their own.
        assert!(!svg.contains("2026-11-08"), "{svg}");
        assert!(!svg.contains("2026-11-22"), "{svg}");
    }

    #[test]
    fn the_dates_of_a_trend_stay_inside_the_chart() {
        let svg = render_svg(&five_weeks());

        // The first date starts at its point, the last one ends at its point.
        assert!(svg.contains("text-anchor='start' font-size='8' fill='#666' font-family='Inter, sans-serif'>2026-11-01<"), "{svg}");
        assert!(svg.contains("text-anchor='middle' font-size='8' fill='#666' font-family='Inter, sans-serif'>2026-11-15<"), "{svg}");
        assert!(svg.contains("text-anchor='end' font-size='8' fill='#666' font-family='Inter, sans-serif'>2026-11-29<"), "{svg}");
    }

    #[test]
    fn a_trend_inside_one_day_shows_that_day_once() {
        let hour = DAY / 24;
        let points = vec![
            (NOV_1, 1.0),
            (NOV_1 + 6 * hour, 2.0),
            (NOV_1 + 12 * hour, 3.0),
        ];

        let svg = render_svg(&trend_of(vec![("runs".into(), points)]));

        assert_eq!(svg.matches(">2026-11-01</text>").count(), 1, "{svg}");
    }

    #[test]
    fn a_trend_of_one_point_shows_the_date_of_that_point() {
        let svg = render_svg(&trend_of(vec![("score".into(), vec![(NOV_1, 87.0)])]));

        assert_eq!(x_of_text(&svg, "2026-11-01"), marker_xs(&svg)[0]);
    }

    #[test]
    fn the_dates_of_a_trend_are_those_of_all_its_series() {
        let svg = render_svg(&trend_of(vec![
            ("early".into(), vec![(NOV_1, 1.0), (NOV_1 + DAY, 2.0)]),
            (
                "late".into(),
                vec![(NOV_1 + 3 * DAY, 3.0), (NOV_1 + 4 * DAY, 4.0)],
            ),
        ]));

        // Four instants: the first, the third and the last have their date.
        assert!(svg.contains(">2026-11-01</text>"), "{svg}");
        assert!(svg.contains(">2026-11-04</text>"), "{svg}");
        assert!(svg.contains(">2026-11-05</text>"), "{svg}");
        assert!(!svg.contains(">2026-11-02</text>"), "{svg}");
    }

    #[test]
    fn each_series_of_a_trend_has_its_own_colour_until_the_palette_starts_again() {
        let series = (0..=PALETTE.len())
            .map(|number| (format!("s{number}"), vec![(NOV_1, 1.0), (NOV_1 + DAY, 2.0)]))
            .collect();

        let svg = render_svg(&trend_of(series));

        let strokes: Vec<&str> = svg
            .split("<polyline")
            .skip(1)
            .map(|line| {
                let stroke = line.split("stroke='").nth(1).expect("a stroke");
                &stroke[..stroke.find('\'').expect("the end of the stroke")]
            })
            .collect();
        assert_eq!(strokes[..PALETTE.len()], PALETTE);
        assert_eq!(strokes[PALETTE.len()], PALETTE[0]);
    }

    /// The points of the line of `svg`, as text.
    fn line_points(svg: &str) -> Vec<String> {
        let line = svg.split("<polyline points='").nth(1).expect("a line");
        line[..line.find('\'').expect("the end of the points")]
            .split(' ')
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_sparkline_runs_from_its_lowest_to_its_highest_value_across_its_canvas() {
        let points = vec![(NOV_1, 2.0), (NOV_1 + DAY, 4.0), (NOV_1 + 2 * DAY, 6.0)];

        let svg = render_svg(&ChartSpec::Sparkline(points));

        // 120 by 28 with 3 at each side: the lowest value is at the bottom
        // left, the highest at the top right, the middle one in the middle.
        assert!(svg.contains("width='120' height='28'"), "{svg}");
        assert_eq!(line_points(&svg), ["3.0,25.0", "60.0,14.0", "117.0,3.0"]);
    }

    #[test]
    fn a_sparkline_places_its_points_by_their_time() {
        // The second point is a quarter of the way, not half of it.
        let points = vec![(NOV_1, 0.0), (NOV_1 + DAY, 1.0), (NOV_1 + 4 * DAY, 4.0)];

        let svg = render_svg(&ChartSpec::Sparkline(points));

        assert_eq!(line_points(&svg), ["3.0,25.0", "31.5,19.5", "117.0,3.0"]);
    }

    #[test]
    fn a_sparkline_marks_its_last_point_and_has_no_text() {
        let points = vec![(NOV_1, 5.0), (NOV_1 + DAY, 1.0)];

        let svg = render_svg(&ChartSpec::Sparkline(points));

        assert_eq!(svg.matches("<circle").count(), 1, "{svg}");
        assert!(svg.contains("<circle cx='117.0' cy='25.0'"), "{svg}");
        assert!(!svg.contains("<text"), "{svg}");
    }

    #[test]
    fn a_sparkline_is_a_thin_line_in_the_first_colour_with_a_small_marker() {
        let points = vec![(NOV_1, 5.0), (NOV_1 + DAY, 1.0)];

        let svg = render_svg(&ChartSpec::Sparkline(points));

        let first = PALETTE[0];
        assert!(
            svg.contains(&format!(
                "fill='none' stroke='{first}' stroke-width='1.4'/>"
            )),
            "{svg}"
        );
        assert!(svg.contains(&format!(" r='2' fill='{first}'/>")), "{svg}");
    }

    #[test]
    fn a_sparkline_of_equal_values_is_a_level_line_in_the_middle() {
        let points = vec![(NOV_1, 7.0), (NOV_1 + DAY, 7.0)];

        let svg = render_svg(&ChartSpec::Sparkline(points));

        assert_eq!(line_points(&svg), ["3.0,14.0", "117.0,14.0"]);
    }

    #[test]
    fn a_sparkline_of_one_point_is_its_marker_at_the_end() {
        let svg = render_svg(&ChartSpec::Sparkline(vec![(NOV_1, 7.0)]));

        assert!(!svg.contains("<polyline"), "{svg}");
        assert!(svg.contains("<circle cx='117.0' cy='14.0'"), "{svg}");
    }

    #[test]
    fn a_sparkline_without_a_point_renders_nothing() {
        assert!(render_svg(&ChartSpec::Sparkline(Vec::new())).is_empty());
    }

    #[test]
    fn fmt_num_trims_but_keeps_significant_decimals() {
        assert_eq!(fmt_num(75.0), "75");
        assert_eq!(fmt_num(2.5), "2.5");
        assert_eq!(fmt_num(-4.0), "-4");
        assert_eq!(fmt_num(0.26), "0.3"); // one decimal, rounded
    }

    #[test]
    fn esc_covers_every_special() {
        assert_eq!(
            esc("<a href=\"x\">&amp;</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;amp;&lt;/a&gt;"
        );
    }
}

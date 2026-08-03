//! Minimal hand-rolled SVG charts (bars, donut, lines) in the Okabe-Ito
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
        ChartSpec::Lines(series) => lines(series),
    }
}

/// Horizontal bars in caller order, with direct value labels at bar ends.
fn bars(rows: &[(String, f64)]) -> String {
    let label_w = 170.0_f64;
    let bar_max_w = 240.0_f64;
    let row_h = 22.0_f64;
    let max = rows.iter().map(|r| r.1).fold(0.0_f64, f64::max).max(1e-9);
    let h = rows.len() as f64 * row_h + 8.0;
    let mut out = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='480' height='{h:.0}' viewBox='0 0 480 {h:.0}'>"
    );
    for (i, (label, v)) in rows.iter().enumerate() {
        let y = i as f64 * row_h + 4.0;
        let w = (v / max * bar_max_w).max(1.0);
        let color = PALETTE[i % PALETTE.len()];
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

/// Multi-series lines with direct end-of-line series labels.
fn lines(series: &[(String, Vec<(f64, f64)>)]) -> String {
    let (w, h) = (480.0_f64, 160.0);
    let (ml, mr, mt, mb) = (40.0, 90.0, 10.0, 22.0);
    let xs: Vec<f64> = series
        .iter()
        .flat_map(|s| s.1.iter().map(|p| p.0))
        .collect();
    let ys: Vec<f64> = series
        .iter()
        .flat_map(|s| s.1.iter().map(|p| p.1))
        .collect();
    if xs.is_empty() || ys.is_empty() {
        return String::new();
    }
    let (x0, x1) = (
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    let (y0, y1) = (
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    // Pad the range so the trend shape is readable (no forced zero
    // baseline — that flattens any score trend into a straight line).
    // Non-negative series (scores, counts) never dip below zero though.
    let pad = ((y1 - y0) * 0.15).max(2.0);
    let y0 = if y0 >= 0.0 {
        (y0 - pad).max(0.0)
    } else {
        y0 - pad
    };
    let (y0, mut y1) = (y0, y1 + pad);
    if (y1 - y0).abs() < 1e-9 {
        y1 = y0 + 1.0;
    }
    let sx = |x: f64| ml + (x - x0) / (x1 - x0).max(1e-9) * (w - ml - mr);
    let sy = |y: f64| mt + (1.0 - (y - y0) / (y1 - y0)) * (h - mt - mb);
    let mut out = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{w:.0}' height='{h:.0}' viewBox='0 0 {w:.0} {h:.0}'>"
    );
    // Axes + max/min y labels.
    out.push_str(&format!(
        "<line x1='{ml}' y1='{zy}' x2='{ex}' y2='{zy}' stroke='#999' stroke-width='0.7'/>\
         <line x1='{ml}' y1='{mt}' x2='{ml}' y2='{zy}' stroke='#999' stroke-width='0.7'/>\
         <text x='2' y='{my:.1}' font-size='8' fill='#666' {FONT}>{y1}</text>\
         <text x='2' y='{zy:.1}' font-size='8' fill='#666' {FONT}>{y0}</text>",
        zy = sy(y0),
        ex = w - mr,
        my = sy(y1) + 7.0,
        y1 = fmt_num(y1),
        y0 = fmt_num(y0),
    ));
    for (i, (name, points)) in series.iter().enumerate() {
        if points.is_empty() {
            continue;
        }
        let color = PALETTE[i % PALETTE.len()];
        let path: Vec<String> = points
            .iter()
            .map(|p| format!("{:.1},{:.1}", sx(p.0), sy(p.1)))
            .collect();
        out.push_str(&format!(
            "<polyline points='{}' fill='none' stroke='{color}' stroke-width='1.6'/>",
            path.join(" ")
        ));
        // Always draw point markers so single-point series are visible.
        for p in points {
            out.push_str(&format!(
                "<circle cx='{:.1}' cy='{:.1}' r='2.2' fill='{color}'/>",
                sx(p.0),
                sy(p.1)
            ));
        }
        let last = points[points.len() - 1];
        out.push_str(&format!(
            "<text x='{:.1}' y='{:.1}' font-size='9' fill='{color}' {FONT}>{}</text>",
            sx(last.0) + 5.0,
            sy(last.1) + 3.0,
            esc(name)
        ));
    }
    out.push_str("</svg>");
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
    fn donut_handles_full_circle_and_empty() {
        let full = render_svg(&ChartSpec::Donut(vec![("passed".into(), 6.0)]));
        assert!(full.contains("<circle"));
        assert!(render_svg(&ChartSpec::Donut(vec![])).is_empty());
    }

    #[test]
    fn single_point_line_still_draws_a_marker() {
        let svg = render_svg(&ChartSpec::Lines(vec![("score".into(), vec![(1.0, 87.0)])]));
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
        let svg = render_svg(&ChartSpec::Lines(vec![
            ("empty".into(), vec![]),
            ("delta".into(), vec![(1.0, -4.0), (2.0, 6.5)]),
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
        assert!(render_svg(&ChartSpec::Lines(vec![])).is_empty());
        assert!(render_svg(&ChartSpec::Lines(vec![("a".into(), vec![])])).is_empty());
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

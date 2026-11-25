// The chart theme of Tumult: the tokens of each theme and what ECharts
// makes of them. No chart is loaded here, so that the UI and the script of
// the CLI report each load only their own.

import { tokens } from './tokens.gen';

export type ChartTheme = 'tumult-dark' | 'tumult-light';

/**
 * What a chart draws in, from the roles of a theme: the same values the
 * pages take from tokens.css, and the series and type of charts. Generic,
 * so that DARK and LIGHT keep each value as its type.
 */
function chartRoles<T extends (typeof tokens)['dark' | 'light']>(t: T) {
    return {
        text: t['text-dim'],
        axis: t['border-strong'],
        split: t.border,
        accent: t.accent,
        ok: t.ok,
        warn: t.warn,
        fail: t.fail,
        series: t.chart.series,
        extraSeries: t.chart['extra-series'],
        font: { ...t.chart.font, mono: t.mono },
        tooltip: {
            backgroundColor: t['bg-panel'],
            borderColor: t['border-strong'],
            textStyle: { color: t.text, fontSize: t.chart.font.size.md },
        },
    } as const;
}

export const DARK = chartRoles(tokens.dark);
export const LIGHT = chartRoles(tokens.light);

// Series palette (Okabe-Ito, colour-blind safe); status hues stay reserved.
// PALETTE is the one of the dark theme; `palette` gives the one of a theme.
export const PALETTE: string[] = [...DARK.series];

/** The series colours of `theme`, in order. Each is 3:1 or more on its panel. */
export const palette = (theme: ChartTheme): string[] => [
    ...(theme === 'tumult-light' ? LIGHT : DARK).series,
];

export function themeSpec(t: typeof DARK | typeof LIGHT) {
    const axis = {
        axisLine: { lineStyle: { color: t.axis } },
        axisTick: { lineStyle: { color: t.axis } },
        axisLabel: { color: t.text },
        splitLine: { lineStyle: { color: t.split } },
    };
    return {
        color: [...t.series],
        backgroundColor: 'transparent',
        textStyle: { color: t.text, fontFamily: t.font.family },
        title: { textStyle: { color: t.text } },
        legend: { textStyle: { color: t.text } },
        tooltip: t.tooltip,
        categoryAxis: axis,
        valueAxis: axis,
        timeAxis: axis,
        logAxis: axis,
        line: { smooth: false, symbol: 'none', lineStyle: { width: 1.6 } },
    };
}

// The chart colours that are the same in every theme. The semantic tier
// holds them, so the dark and the light tokens have the same values.
const shared = tokens.dark.chart;

/** Text on a bright fill, such as a heatmap cell or a treemap tile. */
export const INK_ON_FILL: string = shared['ink-on-fill'];
/** The dot of a tooltip row whose colour is not one CSS can write. */
export const NEUTRAL_DOT: string = shared['neutral-dot'];
/** The bands of a score, and the coverage of a DORA cell from low to high. */
export const BAND = shared.band;
/** A band with no score. */
export const UNSCORED: string = shared.unscored;
/** The evidence status of a DORA cell, by rank. */
export const STATUS = shared.status;
/** The fills of the Star Map kinds and the lines of its relations. */
export const STARMAP = shared.starmap;

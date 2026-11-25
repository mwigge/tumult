// The chart theme of Tumult: the tokens of each theme and what ECharts
// makes of them. No chart is loaded here, so that the UI and the script of
// the CLI report each load only their own.

import { tokens } from './tokens.gen';

export type ChartTheme = 'tumult-dark' | 'tumult-light';

/**
 * What a chart draws in, from the colour roles of a theme: the same values
 * the pages take from tokens.css. Generic, so that DARK and LIGHT keep each
 * colour as its type.
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
        tooltip: {
            backgroundColor: t['bg-panel'],
            borderColor: t['border-strong'],
            textStyle: { color: t.text, fontSize: 12 },
        },
    } as const;
}

export const DARK = chartRoles(tokens.dark);
export const LIGHT = chartRoles(tokens.light);

// Series palette (Okabe-Ito, colour-blind safe); status hues stay reserved.
export const PALETTE: string[] = [...tokens.dark.chart.series];

export function themeSpec(t: typeof DARK | typeof LIGHT) {
    const axis = {
        axisLine: { lineStyle: { color: t.axis } },
        axisTick: { lineStyle: { color: t.axis } },
        axisLabel: { color: t.text },
        splitLine: { lineStyle: { color: t.split } },
    };
    return {
        color: PALETTE,
        backgroundColor: 'transparent',
        textStyle: { color: t.text },
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

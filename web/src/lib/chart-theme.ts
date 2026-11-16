// The chart theme of Tumult: the tokens of each theme and what ECharts
// makes of them. No chart is loaded here, so that the UI and the script of
// the CLI report each load only their own.

// Chart tokens per theme, matching the variables in lib/theme.css.
export type ChartTheme = 'tumult-dark' | 'tumult-light';

export const DARK = {
    text: '#8b98a5',
    axis: '#33404c',
    split: '#242d36',
    accent: '#5eb1ef',
    ok: '#34d399',
    warn: '#f59e0b',
    fail: '#ef4444',
    tooltip: {
        backgroundColor: '#151b21',
        borderColor: '#33404c',
        textStyle: { color: '#d8dee4', fontSize: 12 },
    },
} as const;

export const LIGHT = {
    text: '#5b6772',
    axis: '#c5ced6',
    split: '#e3e8ed',
    accent: '#1f6fb2',
    ok: '#0f8a5f',
    warn: '#b45309',
    fail: '#c0392b',
    tooltip: {
        backgroundColor: '#ffffff',
        borderColor: '#c5ced6',
        textStyle: { color: '#1b2329', fontSize: 12 },
    },
} as const;

// Series palette (Okabe-Ito, colour-blind safe); status hues stay reserved.
export const PALETTE = [
    '#5eb1ef',
    '#E69F00',
    '#009E73',
    '#CC79A7',
    '#56B4E9',
    '#D55E00',
    '#F0E442',
];

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

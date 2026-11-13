// Tree-shaken ECharts: only the charts/components the UI uses.
import * as echarts from 'echarts/core';
import {
    BarChart,
    CustomChart,
    GaugeChart,
    GraphChart,
    HeatmapChart,
    LineChart,
    PieChart,
    ScatterChart,
    TreemapChart,
} from 'echarts/charts';
import {
    CalendarComponent,
    DataZoomComponent,
    GridComponent,
    LegendComponent,
    MarkAreaComponent,
    MarkLineComponent,
    TooltipComponent,
    VisualMapComponent,
} from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';

echarts.use([
    LineChart,
    BarChart,
    PieChart,
    HeatmapChart,
    ScatterChart,
    GraphChart,
    TreemapChart,
    CustomChart,
    GaugeChart,
    GridComponent,
    TooltipComponent,
    DataZoomComponent,
    CalendarComponent,
    VisualMapComponent,
    LegendComponent,
    MarkAreaComponent,
    MarkLineComponent,
    CanvasRenderer,
]);

export default echarts;
export type { EChartsCoreOption } from 'echarts/core';

const HTML_ESCAPES: Record<string, string> = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
};

/**
 * Text for an HTML tooltip. ECharts sets the string a `formatter` returns
 * as markup, so every value that goes into one passes through here: names
 * and labels come from telemetry and from files, not from this code.
 */
export function escapeHtml(value: unknown): string {
    return String(value ?? '').replace(/[&<>"']/g, (c) => HTML_ESCAPES[c]);
}

// Chart tokens per theme, matching the variables in lib/theme.css.
export type ChartTheme = 'tumult-dark' | 'tumult-light';

const DARK = {
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

const LIGHT = {
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
const PALETTE = ['#5eb1ef', '#E69F00', '#009E73', '#CC79A7', '#56B4E9', '#D55E00', '#F0E442'];

function themeSpec(t: typeof DARK | typeof LIGHT) {
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

echarts.registerTheme('tumult-dark', themeSpec(DARK));
echarts.registerTheme('tumult-light', themeSpec(LIGHT));

/**
 * The theme in force: `data-theme` on <html> wins (`dark` | `light`);
 * `auto` or unset follows `prefers-color-scheme`, and the UI defaults to
 * dark when nothing is set (see lib/theme.css).
 */
export function chartTheme(): ChartTheme {
    if (typeof document === 'undefined') return 'tumult-dark';
    const pref = document.documentElement.dataset.theme;
    if (pref === 'light') return 'tumult-light';
    if (pref === 'dark' || pref === undefined) return 'tumult-dark';
    return window.matchMedia?.('(prefers-color-scheme: light)').matches
        ? 'tumult-light'
        : 'tumult-dark';
}

/** Colour tokens for the theme in force (for series and marks). */
export function chartTokens(theme: ChartTheme = chartTheme()) {
    return theme === 'tumult-light' ? LIGHT : DARK;
}

// Dark tokens, kept for the pages that still build options by hand.
export const CHART = {
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

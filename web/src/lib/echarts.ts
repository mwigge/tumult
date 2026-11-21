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
    RadarChart,
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
    RadarComponent,
    TooltipComponent,
    VisualMapComponent,
} from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';
import { DARK, LIGHT, themeSpec } from './chart-theme';
import type { ChartTheme } from './chart-theme';

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
    RadarChart,
    GridComponent,
    RadarComponent,
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
export { PALETTE } from './chart-theme';
export type { ChartTheme } from './chart-theme';

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

// The surfaces a chart of the UI draws on, per theme (lib/theme.css). They
// are kept here, and not in chart-theme.ts, so that the script of the CLI
// report stays the same.
const SURFACES = {
    'tumult-dark': { bg: '#0b0e11', panel: '#151b21', hover: '#1c242c', accentDim: '#2d5d84' },
    'tumult-light': { bg: '#f4f6f8', panel: '#ffffff', hover: '#e9eef2', accentDim: '#9cc4e4' },
} as const;

/** Colour tokens for the theme in force (for series and marks). */
export function chartTokens(theme: ChartTheme = chartTheme()) {
    return { ...(theme === 'tumult-light' ? LIGHT : DARK), ...SURFACES[theme] };
}

/**
 * Call `onchange` with the theme in force now and each time it changes.
 * A page keeps the theme in a `$state`, so that it builds the options of
 * its charts again in the new colours. Returns what stops it.
 */
export function followTheme(onchange: (theme: ChartTheme) => void): () => void {
    const update = () => onchange(chartTheme());
    update();
    const mo = new MutationObserver(update);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    const mq = window.matchMedia?.('(prefers-color-scheme: light)');
    mq?.addEventListener?.('change', update);
    return () => {
        mo.disconnect();
        mq?.removeEventListener?.('change', update);
    };
}

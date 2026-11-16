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

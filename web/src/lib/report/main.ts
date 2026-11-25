// The script of the CLI HTML report (`tumult report`). The report carries
// it inline, so a report is one file that draws its charts with no network.
//
// A chart of the report is an element with an id and its height in
// `data-height`, and next to it a
// `<script type="application/json" data-chart="<id>">` with its ECharts
// option. This script draws each one in the Tumult theme the browser asks
// for, and again when that changes.
//
// Built by `npm run build:report` into
// `tumult-cli/src/commands/report/html/report-charts.js`.
import * as echarts from 'echarts/core';
import { BarChart, LineChart } from 'echarts/charts';
import {
    GridComponent,
    LegendComponent,
    MarkAreaComponent,
    TooltipComponent,
} from 'echarts/components';
import { SVGRenderer } from 'echarts/renderers';
import { DARK, LIGHT, themeSpec } from '../chart-theme';
import { COLOURS, withColours } from './colours';

echarts.use([
    BarChart,
    LineChart,
    GridComponent,
    LegendComponent,
    MarkAreaComponent,
    TooltipComponent,
    SVGRenderer,
]);
echarts.registerTheme('tumult-dark', themeSpec(DARK));
echarts.registerTheme('tumult-light', themeSpec(LIGHT));

const dark = window.matchMedia('(prefers-color-scheme: dark)');

function draw(spec: HTMLScriptElement): void {
    const host = document.getElementById(spec.dataset.chart ?? '');
    if (!host) return;
    // A report that is read without its script has no empty space for a chart.
    host.style.height = `${host.dataset.height}px`;
    const option: unknown = JSON.parse(spec.textContent ?? '');
    let chart: echarts.ECharts | undefined;
    const render = () => {
        chart?.dispose();
        chart = echarts.init(host, dark.matches ? 'tumult-dark' : 'tumult-light', {
            renderer: 'svg',
        });
        chart.setOption(
            withColours(
                option,
                COLOURS[dark.matches ? 'dark' : 'light'],
            ) as echarts.EChartsCoreOption,
        );
    };
    render();
    dark.addEventListener('change', render);
    window.addEventListener('resize', () => chart?.resize());
}

document.querySelectorAll<HTMLScriptElement>('script[data-chart]').forEach(draw);

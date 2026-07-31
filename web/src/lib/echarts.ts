// Tree-shaken ECharts: only the charts/components the UI uses.
import * as echarts from 'echarts/core';
import { BarChart, GraphChart, HeatmapChart, LineChart, PieChart, ScatterChart, TreemapChart } from 'echarts/charts';
import {
  CalendarComponent,
  DataZoomComponent,
  GridComponent,
  LegendComponent,
  MarkAreaComponent,
  MarkLineComponent,
  TooltipComponent,
  VisualMapComponent
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
  GridComponent,
  TooltipComponent,
  DataZoomComponent,
  CalendarComponent,
  VisualMapComponent,
  LegendComponent,
  MarkAreaComponent,
  MarkLineComponent,
  CanvasRenderer
]);

export default echarts;
export type { EChartsCoreOption } from 'echarts/core';

// Shared dark-chart defaults, matching lib/theme.css.
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
    textStyle: { color: '#d8dee4', fontSize: 12 }
  }
} as const;

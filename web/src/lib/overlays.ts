// Experiment-run chart overlays (research pattern 2): runs overlapping the
// visible window render as outcome-coloured markArea bands plus a markLine
// at each run's start; clicking a band/line navigates to the run.
//
// One helper, used by the Overview and Metrics pages. Attach the returned
// series to any chart with a time x-axis (for a category axis, pass `toX`
// mapping ms → the category label, e.g. the day string).

import { CHART } from '$lib/echarts';
import type { EChartsCoreOption } from '$lib/echarts';
import type { ExperimentWindow } from '$lib/types';

type Series =
  EChartsCoreOption['series'] extends Array<infer S> ? S : Record<string, unknown>;

/** Outcome → band color (theme Okabe-Ito hues). */
export function outcomeColor(outcome: string | null): string {
  switch ((outcome ?? '').toLowerCase()) {
    case 'completed':
      return CHART.ok;
    case 'deviated':
      return CHART.warn;
    case 'failed':
      return CHART.fail;
    default:
      return CHART.text;
  }
}

const shortName = (r: ExperimentWindow) => r.name ?? r.id ?? '(run)';

/**
 * A silent series carrying the overlay. `toX` converts epoch ms to the
 * x-axis value (identity for time axes; day-string for category axes).
 */
export function experimentOverlay(
  runs: ExperimentWindow[],
  toX: (ms: number) => number | string = (ms) => ms
): Series | null {
  if (runs.length === 0) return null;
  return {
    type: 'line',
    data: [],
    silent: false,
    markArea: {
      silent: false,
      data: runs.map((r) => [
        {
          xAxis: toX(r.start_ns / 1e6),
          runId: r.id,
          itemStyle: { color: `${outcomeColor(r.outcome)}1a` } // ~10% opacity
        },
        { xAxis: toX(r.end_ns / 1e6) }
      ])
    },
    markLine: {
      silent: false,
      symbol: 'none',
      label: {
        show: true,
        position: 'insideEndTop',
        fontSize: 10,
        color: CHART.text,
        formatter: (p: { name?: string }) => p.name ?? ''
      },
      data: runs.map((r) => ({
        xAxis: toX(r.start_ns / 1e6),
        name: `${shortName(r)} — ${r.outcome ?? 'unknown'}`,
        runId: r.id,
        lineStyle: { color: outcomeColor(r.outcome), type: 'dashed', width: 1 }
      }))
    }
  } as Series;
}

/** Extract the experiment id from an ECharts click on a markArea/markLine. */
export function overlayRunId(params: { componentType?: string; data?: unknown }): string | null {
  if (params.componentType !== 'markArea' && params.componentType !== 'markLine') return null;
  const d = params.data;
  // markArea clicks report the first edge object; markLine clicks the datum.
  const id =
    d && typeof d === 'object' && !Array.isArray(d)
      ? (d as { runId?: string | null }).runId
      : Array.isArray(d)
        ? (d[0] as { runId?: string | null } | undefined)?.runId
        : null;
  return id ?? null;
}

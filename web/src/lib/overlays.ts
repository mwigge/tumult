// Experiment-run chart overlays (research pattern 2): runs overlapping the
// visible window render as outcome-coloured markArea bands plus a markLine
// at each run's start; clicking a band/line navigates to the run.
//
// One helper, used by the Overview and Metrics pages. Attach the returned
// series to any chart with a time x-axis (for a category axis, pass `toX`
// mapping ms → the category label, e.g. the day string).

import { CHART } from '#lib/echarts.js';
import type { EChartsCoreOption } from '#lib/echarts.js';
import type { ExperimentWindow } from '#lib/types.js';

type Series = EChartsCoreOption['series'] extends Array<infer S> ? S : Record<string, unknown>;

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

type X = number | string;

/** Runs that start on the same x value (one day column, one instant). */
function groupByX(runs: ExperimentWindow[], toX: (ms: number) => X): Map<X, ExperimentWindow[]> {
    const groups = new Map<X, ExperimentWindow[]>();
    for (const r of runs) {
        const x = toX(r.start_ns / 1e6);
        groups.set(x, [...(groups.get(x) ?? []), r]);
    }
    return groups;
}

/** One start marker per x: a lone run is named, a group is counted. */
function startMarker(x: X, group: ExperimentWindow[]): Record<string, unknown> {
    const colors = new Set(group.map((r) => outcomeColor(r.outcome)));
    const color = colors.size === 1 ? [...colors][0] : CHART.text;
    const base = { xAxis: x, lineStyle: { color, type: 'dashed', width: 1 } };
    const [only] = group;
    if (group.length !== 1 || only === undefined) {
        return { ...base, name: `${group.length} runs`, runId: null };
    }
    return { ...base, name: `${shortName(only)} — ${only.outcome ?? 'unknown'}`, runId: only.id };
}

/**
 * A silent series carrying the overlay. `toX` converts epoch ms to the
 * x-axis value (identity for time axes; day-string for category axes).
 */
export function experimentOverlay(
    runs: ExperimentWindow[],
    toX: (ms: number) => X = (ms) => ms,
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
                    itemStyle: { color: `${outcomeColor(r.outcome)}1a` }, // ~10% opacity
                },
                { xAxis: toX(r.end_ns / 1e6) },
            ]),
        },
        markLine: {
            silent: false,
            symbol: 'none',
            // Upright above the line: a label inside a vertical line turns
            // with it, and runs on one x drew their names over each other.
            label: {
                show: true,
                position: 'end',
                fontSize: 10,
                color: CHART.text,
                width: 160,
                overflow: 'truncate',
                formatter: (p: { name?: string }) => p.name ?? '',
            },
            data: [...groupByX(runs, toX)].map(([x, group]) => startMarker(x, group)),
        },
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

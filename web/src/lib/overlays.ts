// Experiment-run chart overlays (research pattern 2): runs overlapping the
// visible window render as outcome-coloured markArea bands plus a markLine
// at each run's start; clicking a band/line navigates to the run.
//
// One helper, used by the Overview and Metrics pages. Attach the returned
// series to any chart with a time x-axis (for a category axis, pass `toX`
// mapping ms → the category label, e.g. the day string). The colours come
// from the tokens of the theme the chart is drawn in.

import type { chartTokens, EChartsCoreOption } from '#lib/echarts.js';
import type { LegendItem } from '#lib/legend.js';
import { RUN_TONE, runTone, type Tone } from '#lib/tone.js';
import type { ExperimentWindow } from '#lib/types.js';

type Series = EChartsCoreOption['series'] extends Array<infer S> ? S : Record<string, unknown>;

/** The tokens an overlay is drawn with. */
export type OverlayTokens = Pick<
    ReturnType<typeof chartTokens>,
    'ok' | 'warn' | 'fail' | 'text' | 'font'
>;

function toneColor(tone: Tone, t: OverlayTokens): string {
    return tone === 'neutral' ? t.text : t[tone];
}

/** Outcome → band color in the tokens `t`, in the tone of its verdict. */
export function outcomeColor(outcome: string | null, t: OverlayTokens): string {
    return toneColor(runTone(outcome) ?? 'neutral', t);
}

/** What a run that has not ended is called in the legend; its band is neutral. */
const NOT_ENDED = 'not ended';

/**
 * The legend of the band colours: each colour with the statuses of the
 * runs it is drawn for. A run with no status yet has the neutral colour.
 */
export function outcomeLegend(t: OverlayTokens): LegendItem[] {
    const tones: Tone[] = ['ok', 'warn', 'fail', 'neutral'];
    return tones.flatMap((tone) => {
        const statuses = Object.keys(RUN_TONE).filter((status) => RUN_TONE[status] === tone);
        const words = tone === 'neutral' ? [...statuses, NOT_ENDED] : statuses;
        return words.length === 0 ? [] : [{ label: words.join(', '), color: toneColor(tone, t) }];
    });
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
function startMarker(x: X, group: ExperimentWindow[], t: OverlayTokens): Record<string, unknown> {
    const colors = new Set(group.map((r) => outcomeColor(r.outcome, t)));
    const color = colors.size === 1 ? [...colors][0] : t.text;
    const base = { xAxis: x, lineStyle: { color, type: 'dashed', width: 1 } };
    const [only] = group;
    if (group.length !== 1 || only === undefined) {
        return { ...base, name: `${group.length} runs`, runId: null };
    }
    return { ...base, name: `${shortName(only)} — ${only.outcome ?? 'unknown'}`, runId: only.id };
}

/** Share of the plot width that one run label needs (160px, truncated). */
const LABEL_SHARE = 0.15;

/** The smallest distance on a time axis at which two run labels do not touch. */
export function labelGap(xs: number[]): number {
    if (xs.length === 0) return 0;
    const min = xs.reduce((a, b) => Math.min(a, b));
    const max = xs.reduce((a, b) => Math.max(a, b));
    return (max - min) * LABEL_SHARE;
}

/**
 * Which labels to show so that no two are closer than `minGap`: walking
 * left to right, a label is shown when it is clear of the last shown one.
 * The flags keep the order of `xs`.
 */
export function spacedLabels(xs: number[], minGap: number): boolean[] {
    const order = xs.map((_, i) => i).sort((a, b) => (xs[a] ?? 0) - (xs[b] ?? 0));
    const shown = xs.map(() => true);
    let last = -Infinity;
    for (const i of order) {
        const x = xs[i] ?? 0;
        shown[i] = x - last >= minGap;
        if (shown[i]) last = x;
    }
    return shown;
}

/** The ends of a time axis, in epoch ms. */
export interface TimeAxis {
    min: number;
    max: number;
}

/**
 * How to anchor a label so it stays inside the chart: a label is centred
 * on its line, so closer than half a label to an end it would be cut off.
 */
function edgeAlign(x: number, minGap: number, axis: TimeAxis | undefined): 'left' | 'right' | null {
    if (axis === undefined) return null;
    if (axis.max - x < minGap / 2) return 'right';
    if (x - axis.min < minGap / 2) return 'left';
    return null;
}

/**
 * Hide the labels that would run into a neighbour, and keep the shown
 * ones inside `axis`. Lines and names stay.
 */
function thinLabels(
    markers: Record<string, unknown>[],
    minGap: number,
    axis?: TimeAxis,
): Record<string, unknown>[] {
    const xs = markers.map((m) => m.xAxis);
    if (!xs.every((x) => typeof x === 'number')) return markers;
    const shown = spacedLabels(xs, minGap);
    return markers.map((m, i) => {
        if (!shown[i]) return { ...m, label: { show: false } };
        const align = edgeAlign(xs[i] ?? 0, minGap, axis);
        return align === null ? m : { ...m, label: { align } };
    });
}

/**
 * A silent series carrying the overlay, in the tokens `t`. `toX` converts
 * epoch ms to the x-axis value (identity for time axes; day-string for
 * category axes).
 * On a time axis, `labelGap` hides the name of a run that starts closer
 * than that to the last named one; hovering its line still names it.
 * With `axis`, a name at an end of the axis is anchored to stay inside.
 */
export function experimentOverlay(
    runs: ExperimentWindow[],
    t: OverlayTokens,
    toX: (ms: number) => X = (ms) => ms,
    labelGap = 0,
    axis?: TimeAxis,
): Series | null {
    if (runs.length === 0) return null;
    const markers = [...groupByX(runs, toX)].map(([x, group]) => startMarker(x, group, t));
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
                    itemStyle: { color: `${outcomeColor(r.outcome, t)}1a` }, // ~10% opacity
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
                fontSize: t.font.size.xs,
                color: t.text,
                width: 160,
                overflow: 'truncate',
                formatter: (p: { name?: string }) => p.name ?? '',
            },
            data: labelGap > 0 ? thinLabels(markers, labelGap, axis) : markers,
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

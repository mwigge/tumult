// Presentation helpers for the performance-regression page: number
// formatting, the labels of the answer and the distribution chart option.

import type { chartTokens, EChartsCoreOption } from './echarts';
import { swatch, tip, tipFormatter, type Tip, type TipLine } from './tooltip';
import type { PerfQuantile, PerfSide, PerfVerdict } from './types';

export const PERF_QUANTILES: readonly PerfQuantile[] = ['p50', 'p95', 'p99'];
export const PERF_RANGES: readonly string[] = ['24h', '7d', '30d', '90d'];
export const UNTAGGED = 'untagged';

const DASH = '–';

/** A latency in milliseconds with one decimal, without the unit. */
export function fmtMs(value: number | null): string {
    return value === null ? DASH : value.toFixed(1);
}

/** A signed number with one decimal (`+38.2`, `-0.4`). */
export function fmtSigned(value: number): string {
    return `${value >= 0 ? '+' : ''}${value.toFixed(1)}`;
}

/** `+38.2 ms (+412.0 %)`, or a dash when no difference was computed. */
export function fmtDeltaMs(delta: number | null, deltaPct: number | null): string {
    if (delta === null) return DASH;
    const pct = deltaPct === null ? '' : ` (${fmtSigned(deltaPct)} %)`;
    return `${fmtSigned(delta)} ms${pct}`;
}

/** `[36.9, 39.4]`, or a dash when there is no interval. */
export function fmtInterval(low: number | null, high: number | null): string {
    if (low === null || high === null) return DASH;
    return `[${low.toFixed(1)}, ${high.toFixed(1)}]`;
}

/** A 0..1 confidence as a percentage (`0.95` becomes `95 %`). */
export function fmtConfidence(confidence: number): string {
    return `${Math.round(confidence * 1000) / 10} %`;
}

/** Unanswered windows with their share of the planned ones: `6 (25.0 %)`. */
export function fmtUnanswered(windows: PerfSide['windows']): string {
    if (windows.unanswered_share === null) return `${windows.unanswered}`;
    return `${windows.unanswered} (${(windows.unanswered_share * 100).toFixed(1)} %)`;
}

/** A 0..1 share as a percentage (`35.0 %`), or a dash when unknown. */
export function fmtShare(share: number | null): string {
    return share === null ? DASH : `${(share * 100).toFixed(1)} %`;
}

/** `+34.6 percentage points (margin 1.0)`. */
export function fmtShareDelta(delta: number, margin: number): string {
    return `${fmtSigned(delta * 100)} percentage points (margin ${(margin * 100).toFixed(1)})`;
}

/** Whether a run's load ran its whole plan. */
export function planLabel(run: PerfSide['run_details'][number]): string {
    if (run.aborted) return 'aborted';
    return run.truncated ? 'truncated' : 'whole';
}

export function envLabel(env: string | null): string {
    return env ?? UNTAGGED;
}

export function selectionLabel(selection: PerfVerdict['selection']): string {
    return selection === 'explicit' ? 'explicit versions' : 'deployment order';
}

export interface LabelledRun {
    role: LabelledSide['role'];
    run: PerfSide['run_details'][number];
}

/** Every run of both sides, baseline first. */
export function perfRuns(answer: PerfVerdict): LabelledRun[] {
    return perfSides(answer).flatMap(({ role, side }) =>
        side.run_details.map((run) => ({ role, run })),
    );
}

/** The versions that were deployed again after a later one. */
export function redeployedVersions(answer: PerfVerdict): string[] {
    return answer.versions_seen.filter((v) => v.redeployed).map((v) => v.version);
}

/** `1 observed, 0 declared, 0 inferred`. */
export function attributionLabel(attribution: PerfSide['attribution']): string {
    return `${attribution.observed} observed, ${attribution.declared} declared, ${attribution.inferred} inferred`;
}

export interface LabelledSide {
    role: 'baseline' | 'candidate';
    side: PerfSide;
}

/** The sides of an answer in reading order: baseline (when known), candidate. */
export function perfSides(answer: PerfVerdict): LabelledSide[] {
    const candidate: LabelledSide = { role: 'candidate', side: answer.candidate };
    if (!answer.baseline) return [candidate];
    return [{ role: 'baseline', side: answer.baseline }, candidate];
}

/** True when any run's version on either side is inferred from span timing. */
export function hasInferredAttribution(answer: PerfVerdict): boolean {
    return perfSides(answer).some(({ side }) => side.attribution.inferred > 0);
}

function sideSummary({ role, side }: LabelledSide, quantile: PerfQuantile): string {
    const value = side[quantile];
    const at = value === null ? 'has no samples' : `${quantile} ${fmtMs(value)} ms`;
    return `${role} ${side.version} ${at}`;
}

/** The accessible name of the distribution chart. */
export function perfChartLabel(answer: PerfVerdict): string {
    const quantile = answer.comparison.quantile;
    const parts = perfSides(answer).map((s) => sideSummary(s, quantile));
    const tail = answer.baseline ? '' : ' (no baseline version)';
    return `Latency distribution: ${parts.join(' versus ')}${tail}`;
}

/** The chart tokens the distribution chart reads. */
export type PerfChartTokens = Pick<ReturnType<typeof chartTokens>, 'text' | 'axis' | 'tooltip'>;

interface TooltipPoint {
    seriesName?: string;
    color?: unknown;
    value?: unknown;
}

function tooltipRow(point: TooltipPoint): TipLine {
    const ms = Array.isArray(point.value) ? Number(point.value[1]) : NaN;
    const label = `${point.seriesName ?? ''}: ${Number.isFinite(ms) ? fmtMs(ms) : DASH} ms`;
    return [swatch(point.color), label];
}

/**
 * The tooltip of the distribution chart. A series is named after a
 * version, and a version is whatever a service reported: every line goes
 * through `tip`, which escapes it.
 */
export function perfTooltip(params: unknown): Tip {
    const points = (Array.isArray(params) ? params : [params]) as TooltipPoint[];
    const first = points[0]?.value;
    const head = Array.isArray(first) ? `percentile ${first[0]}` : '';
    return tip(head, ...points.map(tooltipRow));
}

/**
 * Baseline versus candidate percentile curves (x = percentile, y = ms) with
 * a marker at the compared quantile. The baseline is dashed so the two
 * series differ by more than hue.
 */
export function perfDistributionOption(
    answer: PerfVerdict,
    tokens: PerfChartTokens,
): EChartsCoreOption {
    const quantile = answer.comparison.quantile;
    const marker = {
        silent: true,
        symbol: 'none',
        label: { formatter: quantile, color: tokens.text },
        lineStyle: { color: tokens.axis, type: 'dotted' },
        data: [{ xAxis: Number(quantile.slice(1)) }],
    };
    return {
        grid: { left: 52, right: 20, top: 34, bottom: 44 },
        legend: { top: 0 },
        tooltip: {
            ...tokens.tooltip,
            trigger: 'axis',
            className: 'perf-tooltip',
            formatter: tipFormatter(perfTooltip),
        },
        xAxis: {
            type: 'value',
            min: 0,
            max: 100,
            name: 'percentile',
            nameLocation: 'middle',
            nameGap: 26,
        },
        yAxis: { type: 'value', name: 'ms', scale: true },
        series: perfSides(answer).map(({ role, side }) => ({
            name: `${role} ${side.version}`,
            type: 'line',
            showSymbol: false,
            lineStyle: { type: role === 'baseline' ? 'dashed' : 'solid' },
            data: side.distribution.map((d) => [d.p, d.value]),
            ...(role === 'candidate' ? { markLine: marker } : {}),
        })),
    };
}

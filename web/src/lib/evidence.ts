// What the page of a run makes of its evidence: the words, the numbers
// and the options of its charts. The charts are the ones of the CLI
// report (`tumult report`), so a run reads the same in both places.

import type { chartTokens, EChartsCoreOption } from './echarts';
import { swatch, tip, tipFormatter, type Tip, type TipLine } from './tooltip';
import type {
    EvidenceLoad,
    EvidenceObjective,
    EvidenceProbeSample,
    LoadWindow,
    RunEvidence,
} from './types';

/** The chart tokens the charts of a run read. */
export type EvidenceTokens = Pick<ReturnType<typeof chartTokens>, 'fail' | 'tooltip'>;

const DASH = '—';

/**
 * A measured number as short as a person reads it: four digits that
 * count, or every digit of a number of a thousand or more.
 */
export function fmtValue(value: number | null): string {
    if (value === null) return DASH;
    if (Math.abs(value) >= 1000) return String(Math.round(value));
    return String(Number(value.toPrecision(4)));
}

/** The margin of an objective. Room left has a plus; what is over has its minus. */
export function fmtMargin(margin: number | null): string {
    if (margin !== null && margin > 0) return `+${fmtValue(margin)}`;
    return fmtValue(margin);
}

/** How long after the start of the run a step began. One before it is at the start. */
export function stepOffset(run: Pick<RunEvidence, 'started_at_ns'>, ns: number): string {
    return `+${(Math.max(0, ns - run.started_at_ns) / 1e9).toFixed(1)} s`;
}

const PHASES: Record<string, string> = {
    hypothesis_before: 'Steady state before',
    method: 'Method',
    hypothesis_after: 'Steady state after',
    rollback: 'Rollback',
};

/** The name of a phase of the steps. One this page does not know is shown as it is. */
export function phaseLabel(phase: string): string {
    return PHASES[phase] ?? phase;
}

/** What a check of the steady state said. `null`: it was not checked. */
export function steadyState(met: boolean | null): string {
    if (met === null) return 'not checked';
    return met ? 'met' : 'not met';
}

const SIGNS: Record<string, string> = { lte: '≤', gte: '≥', eq: '=' };

/** The target of an objective: `≤ 200`. */
export function objectiveTarget(
    objective: Pick<EvidenceObjective, 'operator' | 'threshold'>,
): string {
    const sign = SIGNS[objective.operator] ?? objective.operator;
    return `${sign} ${fmtValue(objective.threshold)}`;
}

export interface RecoveryRow {
    label: 'RTO' | 'RPO';
    target: string;
    measured: string;
}

const NOT_MEASURED = 'not measured';

function recoveryRow(
    label: RecoveryRow['label'],
    target: number | null,
    measured: number | null,
    unmeasured = NOT_MEASURED,
): RecoveryRow[] {
    if (target === null && measured === null) return [];
    return [
        {
            label,
            target: target === null ? 'not set' : `${fmtValue(target)} s`,
            measured: measured === null ? unmeasured : `${fmtValue(measured)} s`,
        },
    ];
}

/**
 * What the RTO row says when the run measured no recovery time: the time
 * a run that did not recover watched, a lower bound, or the reason of the
 * verdict on the RTO.
 */
function rtoUnmeasured(verdict: EvidenceObjective | undefined): string {
    if (verdict === undefined) return NOT_MEASURED;
    if (verdict.recovered === false && verdict.observed !== null) {
        return `at least ${fmtValue(verdict.observed)} s, not recovered`;
    }
    return verdict.reason ?? NOT_MEASURED;
}

/**
 * The recovery time and the recovery point of a run: each one that has a
 * target or a measure. The RTO row reads the verdict on the RTO the run
 * declares from `objectives`.
 */
export function recoveryRows(
    recovery: Pick<
        RunEvidence['recovery'],
        'rto_target_s' | 'rto_measured_s' | 'rpo_target_s' | 'rpo_measured_s'
    >,
    objectives: EvidenceObjective[] = [],
): RecoveryRow[] {
    const rto = objectives.find((o) => o.nfr_id === 'rto' && o.kind === 'rto');
    return [
        ...recoveryRow('RTO', recovery.rto_target_s, recovery.rto_measured_s, rtoUnmeasured(rto)),
        ...recoveryRow('RPO', recovery.rpo_target_s, recovery.rpo_measured_s),
    ];
}

/** The totals of a load, each with its name, in the units of the summary `tumult run` prints. */
export function loadFacts(load: EvidenceLoad): [string, string][] {
    return [
        ['Tool', load.tool],
        ['Virtual users', String(load.vus)],
        ['Requests', String(load.total_requests)],
        ['Throughput', `${load.throughput_rps.toFixed(1)} requests/s`],
        ['Latency p50', `${load.latency_p50_ms.toFixed(1)} ms`],
        ['Latency p95', `${load.latency_p95_ms.toFixed(1)} ms`],
        ['Latency p99', `${load.latency_p99_ms.toFixed(1)} ms`],
        ['Error rate', `${(load.error_rate * 100).toFixed(2)} %`],
        ['Thresholds', load.thresholds_met ? 'met' : 'not met'],
    ];
}

/** What to say when the page carries fewer samples than the run has. */
export function samplesNote(
    evidence: Pick<RunEvidence, 'probe_samples' | 'probe_samples_total'>,
): string | null {
    const shown = evidence.probe_samples.length;
    if (evidence.probe_samples_total <= shown) return null;
    return `The chart shows the first ${shown} of the ${evidence.probe_samples_total} samples of this run.`;
}

/** The time axis of the charts of a run: from its start, in the unit that fits its length. */
export interface RunAxis {
    startNs: number;
    unit: 's' | 'min' | 'h';
    unitS: number;
}

const TEN_MINUTES_NS = 600e9;
const TEN_HOURS_NS = 60 * TEN_MINUTES_NS;

function axisUnit(lengthNs: number): Pick<RunAxis, 'unit' | 'unitS'> {
    if (lengthNs < TEN_MINUTES_NS) return { unit: 's', unitS: 1 };
    if (lengthNs < TEN_HOURS_NS) return { unit: 'min', unitS: 60 };
    return { unit: 'h', unitS: 3600 };
}

export function runAxis(run: Pick<RunEvidence, 'started_at_ns' | 'ended_at_ns'>): RunAxis {
    return { startNs: run.started_at_ns, ...axisUnit(run.ended_at_ns - run.started_at_ns) };
}

const rounded = (value: number) => Math.round(value * 1000) / 1000;

/** Where the time `ns` is on the axis. A time before the run is at its start. */
export function axisAt(axis: RunAxis, ns: number): number {
    return rounded(Math.max(0, ns - axis.startNs) / 1e9 / axis.unitS);
}

interface TipPoint {
    seriesName?: string;
    color?: unknown;
    value?: unknown;
    data?: unknown;
}

// A sample out of its tolerance is the one point that is not a plain pair.
const isMarked = (point: TipPoint) => point.data !== undefined && !Array.isArray(point.data);

function tipRow(point: TipPoint, unitOf: (series: string) => string): TipLine {
    const series = point.seriesName ?? '';
    const value = Array.isArray(point.value) ? Number(point.value[1]) : NaN;
    const shown = Number.isFinite(value) ? fmtValue(value) : DASH;
    const mark = isMarked(point) ? ', out of tolerance' : '';
    return [swatch(point.color), `${series}: ${shown} ${unitOf(series)}${mark}`];
}

/**
 * The tooltip of a chart of a run: the time, then each series with its
 * value in the unit `unitOf` names for it. A series is named after a
 * probe, and a probe is named in a file: every line goes through `tip`.
 */
export function seriesTip(
    axisUnit: string,
    unitOf: (series: string) => string,
): (params: unknown) => Tip {
    return (params) => {
        const points = (Array.isArray(params) ? params : [params]) as TipPoint[];
        const first = points[0]?.value;
        const head = Array.isArray(first) ? `${first[0]} ${axisUnit}` : '';
        return tip(head, ...points.map((point) => tipRow(point, unitOf)));
    };
}

interface Series {
    type: 'line';
    name: string;
    data: unknown[];
    [key: string]: unknown;
}

/** The time of the samples taken during the fault, as the area a series marks. */
function faultArea(evidence: RunEvidence, axis: RunAxis, tokens: EvidenceTokens): object | null {
    const during = evidence.probe_samples.filter((sample) => sample.phase === 'during');
    if (during.length === 0) return null;
    const from = axisAt(axis, during[0].ts_ns);
    const to = axisAt(axis, during[during.length - 1].ts_ns);
    // One sample is a time without a length: there is no area to mark.
    if (from === to) return null;
    return {
        silent: true,
        itemStyle: { color: tokens.fail, opacity: 0.1 },
        label: { color: tokens.fail },
        data: [[{ name: 'during the fault', xAxis: from }, { xAxis: to }]],
    };
}

/** A chart of lines over the time of the run. The first line marks the fault. */
function lines(
    evidence: RunEvidence,
    axis: RunAxis,
    tokens: EvidenceTokens,
    yAxis: object,
    series: Series[],
    unitOf: (series: string) => string,
): EChartsCoreOption {
    const area = faultArea(evidence, axis, tokens);
    if (area !== null && series.length > 0) series[0].markArea = area;
    return {
        animation: false,
        grid: { left: 56, right: 56, top: 40, bottom: 44 },
        legend: { top: 0 },
        tooltip: {
            trigger: 'axis',
            formatter: tipFormatter(seriesTip(axis.unit, unitOf)),
        },
        xAxis: {
            type: 'value',
            min: 0,
            name: `Time from the start of the run (${axis.unit})`,
            nameLocation: 'middle',
            nameGap: 28,
        },
        yAxis,
        series,
    };
}

/** A sample as a point. One out of its tolerance is larger, and in the colour of a failure. */
function samplePoint(sample: EvidenceProbeSample, axis: RunAxis, tokens: EvidenceTokens): unknown {
    const at = [axisAt(axis, sample.ts_ns), sample.duration_ms];
    if (sample.ok) return at;
    return { value: at, symbol: 'circle', symbolSize: 9, itemStyle: { color: tokens.fail } };
}

/** How long each sample of each probe took, over the time of the run. */
export function probeSamplesOption(
    evidence: RunEvidence,
    tokens: EvidenceTokens,
): EChartsCoreOption {
    const axis = runAxis(evidence);
    const probes = [...new Set(evidence.probe_samples.map((sample) => sample.probe))];
    const series = probes.map((probe): Series => {
        const own = evidence.probe_samples.filter((sample) => sample.probe === probe);
        return {
            type: 'line',
            name: probe,
            data: own.map((sample) => samplePoint(sample, axis, tokens)),
        };
    });
    const yAxis = { type: 'value', name: 'Probe time (ms)' };
    return lines(evidence, axis, tokens, yAxis, series, () => 'ms');
}

/** The windows of the load of a run when they make a chart: two or more. */
function chartWindows(evidence: RunEvidence): LoadWindow[] | null {
    const windows = evidence.load?.windows ?? [];
    return windows.length < 2 ? null : windows;
}

/** A line of few points, each of which is shown. */
function dotted(
    name: string,
    windows: LoadWindow[],
    axis: RunAxis,
    value: (window: LoadWindow) => number | undefined,
): Series {
    return {
        type: 'line',
        name,
        symbol: 'circle',
        symbolSize: 5,
        data: windows.map((window) => [axisAt(axis, window.ts_ns), value(window) ?? null]),
    };
}

/** The latency of the load in each of its windows. Nothing for a load of fewer than two. */
export function loadLatencyOption(
    evidence: RunEvidence,
    tokens: EvidenceTokens,
): EChartsCoreOption | null {
    const windows = chartWindows(evidence);
    if (windows === null) return null;
    const axis = runAxis(evidence);
    const series = [
        dotted('p50', windows, axis, (window) => window.latency_p50_ms),
        dotted('p95', windows, axis, (window) => window.latency_p95_ms),
        dotted('p99', windows, axis, (window) => window.latency_p99_ms),
    ];
    const yAxis = { type: 'value', name: 'Latency (ms)' };
    return lines(evidence, axis, tokens, yAxis, series, () => 'ms');
}

const ERROR_RATE = 'error rate';

const percent = (share: number | undefined) =>
    share === undefined ? undefined : rounded(share * 100);

/** The throughput and the error rate of the load in each of its windows. */
export function loadThroughputOption(
    evidence: RunEvidence,
    tokens: EvidenceTokens,
): EChartsCoreOption | null {
    const windows = chartWindows(evidence);
    if (windows === null) return null;
    const axis = runAxis(evidence);
    const errors = {
        ...dotted(ERROR_RATE, windows, axis, (window) => percent(window.error_rate)),
        yAxisIndex: 1,
        itemStyle: { color: tokens.fail },
        lineStyle: { color: tokens.fail },
    };
    const series = [dotted('throughput', windows, axis, (window) => window.throughput_rps), errors];
    const yAxis = [
        { type: 'value', name: 'Requests/s' },
        { type: 'value', name: 'Errors (%)', min: 0, splitLine: { show: false } },
    ];
    const unitOf = (name: string) => (name === ERROR_RATE ? '%' : 'requests/s');
    return lines(evidence, axis, tokens, yAxis, series, unitOf);
}

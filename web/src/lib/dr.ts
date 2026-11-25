// The Disaster recovery page: how each state, verdict and number of the DR
// view reads, the filters of the page in its URL, and the chart of the RTO
// or RPO of one target over time.

import type { EChartsCoreOption } from './echarts';
import { chartTokens, PALETTE } from './echarts';
import type { ChartTheme } from './chart-theme';
import { strong, tip, tipFormatter } from './tooltip';
import type { Tone } from './tone';
import type { DrLastTest, DrQuery, DrState, DrTarget, DrTest, DrVerdict, DrView } from './types';

const DAY_MS = 86_400_000;

/** How to test DR with Tumult: the disaster recovery guide. */
export const DR_GUIDE_URL = 'https://tumult.rs/guides/disaster-recovery.html';

/** The plans of manual exercises on the Manual page. */
export const PLANS_HREF = '/manual?tab=plans';

/** The manual records on the Manual page. */
export const RECORDS_HREF = '/manual?tab=records';

/** The tests of one page of the table. */
export const PAGE_SIZE = 20;

/** The most tests the charts read: the most one page of the API holds. */
export const CHART_LIMIT = 1000;

/** The days of each period the page offers, shortest first. */
export const PERIOD_DAYS: Readonly<Record<string, number>> = {
    '90d': 90,
    '1y': 365,
    '3y': 3 * 365,
};

export const DEFAULT_PERIOD = '1y';

const STATES: Record<DrState, [string, Tone]> = {
    ok: ['ok', 'ok'],
    due: ['due', 'warn'],
    overdue: ['overdue', 'fail'],
    never_tested: ['never tested', 'neutral'],
};

const VERDICTS: Record<DrVerdict, [string, Tone]> = {
    met: ['met', 'ok'],
    breached: ['breached', 'fail'],
    not_recovered: ['not recovered', 'fail'],
    not_measured: ['not measured', 'neutral'],
};

export const stateLabel = (state: DrState): string => STATES[state][0];
export const stateTone = (state: DrState): Tone => STATES[state][1];
export const verdictLabel = (verdict: DrVerdict): string => VERDICTS[verdict][0];
export const verdictTone = (verdict: DrVerdict): Tone => VERDICTS[verdict][1];

const tenths = (value: number) => String(Math.round(value * 10) / 10);

/** Seconds as a person reads them: `30.1 s` under two minutes, then minutes, then hours. */
export function seconds(s: number): string {
    if (s < 120) return `${tenths(s)} s`;
    if (s < 7200) return `${tenths(s / 60)} min`;
    return `${tenths(s / 3600)} h`;
}

/** A target of an RTO or an RPO, or that there is none. */
export function targetText(s: number | null): string {
    return s === null ? 'no target' : seconds(s);
}

type RtoFacts = Pick<DrTest, 'rto_measured_s' | 'rto_at_least_s' | 'recovered'> & {
    no_outage?: boolean | null;
};

/** What a test measured of its RTO: the time, the least it was when it did not recover, or why there is none. */
export function rtoText(test: RtoFacts): string {
    if (test.recovered === false) {
        if (test.rto_at_least_s === null) return 'not recovered';
        return `at least ${seconds(test.rto_at_least_s)}, not recovered`;
    }
    if (test.rto_measured_s !== null) return seconds(test.rto_measured_s);
    if (test.no_outage) return 'no outage seen';
    return 'not measured';
}

/** What a test measured of its RPO. */
export function rpoText(test: Pick<DrTest, 'rpo_measured_s'>): string {
    return test.rpo_measured_s === null ? 'not measured' : seconds(test.rpo_measured_s);
}

/** Whether an RTO missed `target`: over it, or a run that did not recover. */
export function rtoAlarm(
    test: Pick<DrTest, 'rto_measured_s' | 'recovered'>,
    target: number | null,
): boolean {
    if (test.recovered === false) return true;
    return target !== null && test.rto_measured_s !== null && test.rto_measured_s > target;
}

/** Whether an RPO is over `target`. */
export function rpoAlarm(test: Pick<DrTest, 'rpo_measured_s'>, target: number | null): boolean {
    return target !== null && test.rpo_measured_s !== null && test.rpo_measured_s > target;
}

const counted = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** The targets of the register per state. */
export function registerSummary(states: Record<DrState, number>): string {
    const total = states.ok + states.due + states.overdue + states.never_tested;
    return (
        `${counted(total, 'target')}: ${states.ok} ok, ${states.due} due, ` +
        `${states.overdue} overdue, ${states.never_tested} never tested`
    );
}

/** The tests of the period per origin. */
export function testsSummary(tests: DrView['summary']['tests']): string {
    const sum = (counts: Record<DrVerdict, number>) =>
        counts.met + counts.breached + counts.not_recovered + counts.not_measured;
    return (
        `${counted(tests.total, 'DR test')} in the period: ` +
        `${sum(tests.automated)} automated, ${sum(tests.manual)} manual`
    );
}

/** A time in unix nanoseconds as the day it is where the browser is: `2026-11-23`. */
export function localDay(ns: number): string {
    const date = new Date(ns / 1e6);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** Where a DR test is shown: the page of its run, or the manual records. */
export function testHref(test: Pick<DrTest, 'origin' | 'evidence_id'>): string {
    if (test.origin === 'manual') return RECORDS_HREF;
    return `/experiments/${encodeURIComponent(test.evidence_id)}`;
}

/** Where the last test of a target is shown. Its id is the origin and the evidence id. */
export function lastTestHref(last: DrLastTest): string {
    const evidence = last.id.slice(last.id.indexOf(':') + 1);
    return testHref({ origin: last.origin, evidence_id: evidence });
}

/** The filters of the page, as its URL holds them. */
export interface DrFilters {
    target: string;
    environment: string;
    period: string;
}

export function filtersOf(params: { get(name: string): string | null }): DrFilters {
    const period = params.get('period') ?? '';
    return {
        target: params.get('target') ?? '',
        environment: params.get('environment') ?? '',
        period: Object.hasOwn(PERIOD_DAYS, period) ? period : DEFAULT_PERIOD,
    };
}

/** The query of the filters: the days of the period up to `nowMs`. */
export function viewQuery(filters: DrFilters, nowMs: number): DrQuery {
    const days = PERIOD_DAYS[filters.period] ?? PERIOD_DAYS[DEFAULT_PERIOD];
    return {
        target: filters.target,
        environment: filters.environment,
        from_ns: (nowMs - days * DAY_MS) * 1e6,
        to_ns: nowMs * 1e6,
    };
}

/** Each target system and environment of the register once, sorted: the choices of the filters. */
export function filterOptions(targets: DrTarget[]): { systems: string[]; environments: string[] } {
    const sorted = (values: (string | null)[]) =>
        [...new Set(values.filter((v): v is string => !!v))].sort();
    return {
        systems: sorted(targets.map((t) => t.system)),
        environments: sorted(targets.map((t) => t.environment)),
    };
}

/** The DR tests of one target, oldest first, and the RTO and RPO it is held to. */
export interface RecoveryGroup {
    system: string | null;
    environment: string | null;
    tests: DrTest[];
    rtoTarget: number | null;
    rpoTarget: number | null;
}

/** The newest value of `pick` among `tests`, oldest first. */
function newest(tests: DrTest[], pick: (test: DrTest) => number | null): number | null {
    for (const test of [...tests].reverse()) {
        const value = pick(test);
        if (value !== null) return value;
    }
    return null;
}

/**
 * The tests of each target, by system and environment. A target is held to
 * the RTO and RPO of the register, else to those of its newest test that
 * sets them.
 */
export function recoveryGroups(tests: DrTest[], targets: DrTarget[]): RecoveryGroup[] {
    const groups = new Map<string, RecoveryGroup>();
    const ordered = [...tests].sort(
        (a, b) => a.executed_at_ns - b.executed_at_ns || a.test_id.localeCompare(b.test_id),
    );
    for (const test of ordered) {
        const key = JSON.stringify([test.target_system, test.target_environment]);
        const group = groups.get(key) ?? {
            system: test.target_system,
            environment: test.target_environment,
            tests: [],
            rtoTarget: null,
            rpoTarget: null,
        };
        group.tests.push(test);
        groups.set(key, group);
    }
    return [...groups.values()]
        .map((group) => {
            const held = targets.find(
                (t) => t.system === group.system && t.environment === group.environment,
            );
            return {
                ...group,
                rtoTarget: held?.rto_target_s ?? newest(group.tests, (t) => t.rto_target_s),
                rpoTarget: held?.rpo_target_s ?? newest(group.tests, (t) => t.rpo_target_s),
            };
        })
        .sort(
            (a, b) =>
                (a.system ?? '').localeCompare(b.system ?? '') ||
                (a.environment ?? '').localeCompare(b.environment ?? ''),
        );
}

/** The objective a chart shows. */
export type Objective = 'rto' | 'rpo';

const MEASURED: Record<Objective, (test: DrTest) => number | null> = {
    rto: (test) => test.rto_measured_s,
    rpo: (test) => test.rpo_measured_s,
};

/** The colour of the manual marks: one of the series, not a status. */
export const MANUAL_COLOR = PALETTE[3];

/** The smallest of 1, 2, 2.5 and 5 times a power of ten that is at least `x`. */
function niceCeil(x: number): number {
    if (x <= 0) return 1;
    const power = 10 ** Math.floor(Math.log10(x));
    const step = [1, 2, 2.5, 5, 10].find((m) => m * power >= x) ?? 10;
    return step * power;
}

/** A point of a series: where it is drawn, and which test it is. */
interface Point {
    value: [number, number];
    id: string;
}

const pointOf = (test: DrTest, y: number): Point => ({
    value: [test.executed_at_ns / 1e6, y],
    id: test.test_id,
});

/** Whether the group has anything to draw for `objective`. */
export function hasPoints(group: RecoveryGroup, objective: Objective): boolean {
    return group.tests.some(
        (t) => MEASURED[objective](t) !== null || (objective === 'rto' && t.recovered === false),
    );
}

/** The tooltip of one point: the test, when it ran, what it measured and its verdict. */
function pointTip(test: DrTest, objective: Objective) {
    const origin = test.origin === 'manual' ? `manual ${test.exercise_type ?? ''}` : 'automated';
    const value = objective === 'rto' ? rtoText(test) : rpoText(test);
    return tip(
        strong(test.experiment_name ?? test.test_id),
        [localDay(test.executed_at_ns), ' · ', origin.trim()],
        [objective.toUpperCase(), ': ', strong(value)],
        `verdict ${verdictLabel(test.verdict)}`,
    );
}

/**
 * The RTO or the RPO of one target over the period: a line through what
 * each test measured, the target as a dashed rule, manual tests marked with
 * a diamond and runs that did not recover with a triangle at the least
 * their RTO was.
 */
export function recoveryOption(
    group: RecoveryGroup,
    objective: Objective,
    period: { from_ns: number; to_ns: number },
    theme: ChartTheme,
): EChartsCoreOption {
    const t = chartTokens(theme);
    const target = objective === 'rto' ? group.rtoTarget : group.rpoTarget;
    const measured = group.tests.flatMap((test) => {
        const y = MEASURED[objective](test);
        return y === null ? [] : [{ test, y }];
    });
    const unrecovered =
        objective === 'rto'
            ? group.tests.flatMap((test) =>
                  test.recovered === false && test.rto_at_least_s !== null
                      ? [pointOf(test, test.rto_at_least_s)]
                      : [],
              )
            : [];
    const highest = Math.max(
        target ?? 0,
        ...measured.map((m) => m.y),
        ...unrecovered.map((p) => p.value[1]),
    );
    const line = {
        name: 'measured',
        type: 'line',
        data: measured.map((m) => pointOf(m.test, m.y)),
        symbol: 'circle',
        symbolSize: 6,
        lineStyle: { color: t.accent, width: 1.6 },
        itemStyle: { color: t.accent },
        markLine:
            target === null
                ? undefined
                : {
                      silent: true,
                      symbol: 'none',
                      lineStyle: { color: t.fail, type: 'dashed' },
                      label: {
                          color: t.text,
                          position: 'insideEndTop',
                          formatter: (p: { name?: string }) => p.name ?? '',
                      },
                      data: [{ yAxis: target, name: `target ${seconds(target)}` }],
                  },
    };
    const manual = {
        name: 'manual',
        type: 'scatter',
        data: measured.filter((m) => m.test.origin === 'manual').map((m) => pointOf(m.test, m.y)),
        symbol: 'diamond',
        symbolSize: 12,
        // Above the dots of the line.
        z: 3,
        itemStyle: { color: MANUAL_COLOR },
    };
    const lowerBounds = {
        name: 'not recovered, at least',
        type: 'scatter',
        data: unrecovered,
        symbol: 'triangle',
        symbolSize: 12,
        z: 3,
        itemStyle: { color: t.fail },
    };
    const byId = new Map(group.tests.map((test) => [test.test_id, test]));
    return {
        grid: { left: 52, right: 16, top: 16, bottom: 28 },
        tooltip: {
            trigger: 'item',
            formatter: tipFormatter((p: { data?: { id?: string } }) => {
                const test = byId.get(p.data?.id ?? '');
                return test ? pointTip(test, objective) : null;
            }),
        },
        // A few labels that never overlap: the period starts on any day.
        xAxis: {
            type: 'time',
            min: period.from_ns / 1e6,
            max: period.to_ns / 1e6,
            splitNumber: 4,
            axisLabel: { hideOverlap: true },
        },
        yAxis: { type: 'value', name: 's', min: 0, max: niceCeil(highest * 1.1) },
        series: objective === 'rto' ? [line, manual, lowerBounds] : [line, manual],
    };
}

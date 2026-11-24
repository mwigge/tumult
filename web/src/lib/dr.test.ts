import { describe, expect, it } from 'vitest';

import {
    DR_GUIDE_URL,
    PLANS_HREF,
    filterOptions,
    filtersOf,
    lastTestHref,
    localDay,
    PERIOD_DAYS,
    recoveryGroups,
    registerSummary,
    rpoAlarm,
    rtoAlarm,
    recoveryOption,
    rpoText,
    rtoText,
    seconds,
    stateLabel,
    stateTone,
    targetText,
    testHref,
    testsSummary,
    verdictLabel,
    verdictTone,
    viewQuery,
} from './dr';
import { chartTokens } from './echarts';
import type { DrTarget, DrTest } from './types';

const DAY_MS = 86_400_000;
const NOW_MS = Date.UTC(2026, 10, 23, 12);
const ns = (ms: number) => ms * 1e6;

function drTest(over: Partial<DrTest>): DrTest {
    return {
        test_id: 'automated:run-1',
        origin: 'automated',
        experiment_name: 'ledger failover',
        exercise_type: null,
        target_system: 'ledger',
        target_environment: 'prod',
        executed_at_ns: ns(NOW_MS - 10 * DAY_MS),
        outcome: 'completed',
        rto_target_s: 10,
        rto_measured_s: 4,
        rto_at_least_s: null,
        recovered: true,
        no_outage: false,
        rpo_target_s: null,
        rpo_measured_s: null,
        data_verified: null,
        verdict: 'met',
        evidence_id: 'run-1',
        reviewer: null,
        plan_id: null,
        ...over,
    };
}

function drTarget(over: Partial<DrTarget>): DrTarget {
    return {
        system: 'ledger',
        environment: 'prod',
        state: 'ok',
        last_test: null,
        next_due_at_ns: null,
        interval_days: 365,
        interval_source: 'default',
        plan: null,
        rto_target_s: 10,
        rpo_target_s: null,
        rto_measured_s: null,
        rpo_measured_s: null,
        recovered: null,
        rto_at_least_s: null,
        ...over,
    };
}

describe('seconds', () => {
    it('reads as seconds under two minutes, then minutes, then hours', () => {
        expect([0, 4, 30.1, 30.14, 119].map(seconds)).toEqual([
            '0 s',
            '4 s',
            '30.1 s',
            '30.1 s',
            '119 s',
        ]);
        expect([120, 900, 5400, 7200, 14_400].map(seconds)).toEqual([
            '2 min',
            '15 min',
            '90 min',
            '2 h',
            '4 h',
        ]);
    });
});

describe('what a test measured', () => {
    it('gives the RTO it measured', () => {
        expect(rtoText(drTest({}))).toBe('4 s');
    });

    it('gives the least the RTO was when the run did not recover', () => {
        const run = drTest({ recovered: false, rto_measured_s: null, rto_at_least_s: 30.1 });
        expect(rtoText(run)).toBe('at least 30.1 s, not recovered');
        expect(rtoText({ ...run, rto_at_least_s: null })).toBe('not recovered');
    });

    it('says why a run measured no RTO', () => {
        const quiet = drTest({ rto_measured_s: null, recovered: null, no_outage: true });
        expect(rtoText(quiet)).toBe('no outage seen');
        expect(rtoText({ ...quiet, no_outage: null })).toBe('not measured');
    });

    it('gives the RPO it measured, or says there is none', () => {
        expect(rpoText(drTest({ rpo_measured_s: 10 }))).toBe('10 s');
        expect(rpoText(drTest({}))).toBe('not measured');
    });

    it('names a target, or says there is none', () => {
        expect(targetText(900)).toBe('15 min');
        expect(targetText(null)).toBe('no target');
    });
});

describe('when a number misses its target', () => {
    it('marks an RTO over its target and a run that did not recover', () => {
        expect(rtoAlarm(drTest({ rto_measured_s: 14 }), 10)).toBe(true);
        expect(rtoAlarm(drTest({ rto_measured_s: 10 }), 10)).toBe(false);
        expect(rtoAlarm(drTest({ rto_measured_s: 14 }), null)).toBe(false);
        expect(rtoAlarm(drTest({ rto_measured_s: null, recovered: false }), 10)).toBe(true);
    });

    it('marks an RPO over its target', () => {
        expect(rpoAlarm(drTest({ rpo_measured_s: 61 }), 60)).toBe(true);
        expect(rpoAlarm(drTest({ rpo_measured_s: 60 }), 60)).toBe(false);
        expect(rpoAlarm(drTest({ rpo_measured_s: null }), 60)).toBe(false);
    });
});

describe('the summary of the view', () => {
    it('counts the targets per state', () => {
        expect(registerSummary({ ok: 1, due: 1, overdue: 3, never_tested: 2 })).toBe(
            '7 targets: 1 ok, 1 due, 3 overdue, 2 never tested',
        );
        expect(registerSummary({ ok: 1, due: 0, overdue: 0, never_tested: 0 })).toBe(
            '1 target: 1 ok, 0 due, 0 overdue, 0 never tested',
        );
    });

    it('counts the tests of the period per origin', () => {
        const none = { met: 0, breached: 0, not_recovered: 0, not_measured: 0 };
        const tests = {
            total: 5,
            automated: { met: 1, breached: 1, not_recovered: 1, not_measured: 1 },
            manual: { ...none, met: 1 },
        };
        expect(testsSummary(tests)).toBe('5 DR tests in the period: 4 automated, 1 manual');
        expect(testsSummary({ total: 1, automated: none, manual: { ...none, met: 1 } })).toBe(
            '1 DR test in the period: 0 automated, 1 manual',
        );
    });
});

describe('states and verdicts', () => {
    it('reads each state of the register in words and in its tone', () => {
        const states = ['ok', 'due', 'overdue', 'never_tested'] as const;
        expect(states.map(stateLabel)).toEqual(['ok', 'due', 'overdue', 'never tested']);
        expect(states.map(stateTone)).toEqual(['ok', 'warn', 'fail', 'neutral']);
    });

    it('reads each verdict in words and in its tone', () => {
        const verdicts = ['met', 'breached', 'not_recovered', 'not_measured'] as const;
        expect(verdicts.map(verdictLabel)).toEqual([
            'met',
            'breached',
            'not recovered',
            'not measured',
        ]);
        expect(verdicts.map(verdictTone)).toEqual(['ok', 'fail', 'fail', 'neutral']);
    });
});

describe('the filters of the page', () => {
    it('offer three periods, shortest first', () => {
        expect(Object.entries(PERIOD_DAYS)).toEqual([
            ['90d', 90],
            ['1y', 365],
            ['3y', 1095],
        ]);
    });

    it('come from the URL, with the last year as the period by default', () => {
        const url = new URLSearchParams('target=ledger&environment=prod&period=3y');
        expect(filtersOf(url)).toEqual({ target: 'ledger', environment: 'prod', period: '3y' });
        expect(filtersOf(new URLSearchParams(''))).toEqual({
            target: '',
            environment: '',
            period: '1y',
        });
        expect(filtersOf(new URLSearchParams('period=forever')).period).toBe('1y');
    });

    it('ask the API for the days of the period up to now', () => {
        const query = viewQuery({ target: 'ledger', environment: '', period: '90d' }, NOW_MS);
        expect(query).toEqual({
            target: 'ledger',
            environment: '',
            from_ns: ns(NOW_MS - 90 * DAY_MS),
            to_ns: ns(NOW_MS),
        });
        expect(viewQuery({ target: '', environment: '', period: '1y' }, NOW_MS).from_ns).toBe(
            ns(NOW_MS - 365 * DAY_MS),
        );
        expect(viewQuery({ target: '', environment: '', period: '3y' }, NOW_MS).from_ns).toBe(
            ns(NOW_MS - 3 * 365 * DAY_MS),
        );
    });

    it('offer each target system and environment of the register once, sorted', () => {
        const targets = [
            drTarget({ system: 'orders', environment: 'prod' }),
            drTarget({ system: 'billing', environment: 'staging' }),
            drTarget({ system: 'orders', environment: 'staging' }),
            drTarget({ system: 'archive', environment: null }),
        ];
        expect(filterOptions(targets)).toEqual({
            systems: ['archive', 'billing', 'orders'],
            environments: ['prod', 'staging'],
        });
    });
});

describe('links', () => {
    it('open a run on its page and a manual record among the manual records', () => {
        expect(testHref(drTest({ evidence_id: 'run 1' }))).toBe('/experiments/run%201');
        expect(testHref(drTest({ origin: 'manual', evidence_id: '01ABC' }))).toBe(
            '/manual?tab=records',
        );
        const last = { id: 'automated:run-9', origin: 'automated', executed_at_ns: 1 } as const;
        expect(lastTestHref({ ...last, verdict: 'met' })).toBe('/experiments/run-9');
        expect(
            lastTestHref({ ...last, id: 'manual:01ABC', origin: 'manual', verdict: 'met' }),
        ).toBe('/manual?tab=records');
        expect(PLANS_HREF).toBe('/manual?tab=plans');
    });

    it('lead to the disaster recovery guide of the docs', () => {
        expect(DR_GUIDE_URL).toBe('https://tumult.rs/guides/disaster-recovery.html');
    });

    it('write a day as the date it is where the browser is', () => {
        const ms = new Date(2026, 0, 2, 23, 30).getTime();
        expect(localDay(ns(ms))).toBe('2026-01-02');
    });
});

describe('the recovery of each target', () => {
    const met = drTest({ test_id: 'automated:a', executed_at_ns: ns(NOW_MS - 2 * DAY_MS) });
    const never = drTest({
        test_id: 'automated:b',
        executed_at_ns: ns(NOW_MS - 1 * DAY_MS),
        rto_measured_s: null,
        rto_at_least_s: 30.1,
        recovered: false,
        verdict: 'not_recovered',
    });
    const manual = drTest({
        test_id: 'manual:c',
        origin: 'manual',
        exercise_type: 'failover',
        experiment_name: 'ledger failover drill',
        executed_at_ns: ns(NOW_MS - 30 * DAY_MS),
        rto_target_s: 900,
        rto_measured_s: 600,
        rpo_target_s: 60,
        rpo_measured_s: 20,
    });
    const other = drTest({
        test_id: 'automated:d',
        target_system: 'billing',
        target_environment: 'prod',
    });

    it('keeps the tests of each target together, oldest first, with what it is held to', () => {
        const groups = recoveryGroups(
            [never, other, met, manual],
            [drTarget({ rto_target_s: 20, rpo_target_s: 60 })],
        );
        expect(groups.map((g) => [g.system, g.environment, g.tests.map((t) => t.test_id)])).toEqual(
            [
                ['billing', 'prod', ['automated:d']],
                ['ledger', 'prod', ['manual:c', 'automated:a', 'automated:b']],
            ],
        );
        expect([groups[1].rtoTarget, groups[1].rpoTarget]).toEqual([20, 60]);
        expect([groups[0].rtoTarget, groups[0].rpoTarget]).toEqual([10, null]);
    });

    it('takes the target of the newest test that sets one when the register has none', () => {
        const [group] = recoveryGroups([manual, met], []);
        expect([group.rtoTarget, group.rpoTarget]).toEqual([10, 60]);
    });

    const period = { from_ns: ns(NOW_MS - 365 * DAY_MS), to_ns: ns(NOW_MS) };
    const [ledger] = recoveryGroups([met, never, manual], [drTarget({ rpo_target_s: 60 })]);

    interface Series {
        name: string;
        type: string;
        data: { value: [number, number]; id: string }[];
        markLine?: { data: { yAxis: number; name: string }[] };
        itemStyle?: { color?: string };
    }
    const seriesOf = (option: unknown) => (option as { series: Series[] }).series;
    const points = (s: Series) => s.data.map((d) => d.value);

    it('draws the RTO measured as a line, the target as a rule, and marks manual and unrecovered tests', () => {
        const series = seriesOf(recoveryOption(ledger, 'rto', period, 'tumult-dark'));
        expect(series.map((s) => [s.name, s.type])).toEqual([
            ['measured', 'line'],
            ['manual', 'scatter'],
            ['not recovered, at least', 'scatter'],
        ]);
        const msOf = (t: DrTest) => t.executed_at_ns / 1e6;
        expect(points(series[0])).toEqual([
            [msOf(manual), 600],
            [msOf(met), 4],
        ]);
        expect(points(series[1])).toEqual([[msOf(manual), 600]]);
        expect(points(series[2])).toEqual([[msOf(never), 30.1]]);
        expect(series[0].markLine?.data).toEqual([{ yAxis: 10, name: 'target 10 s' }]);
        expect(series[2].itemStyle?.color).toBe(chartTokens('tumult-dark').fail);
        expect(series.map((s) => (s as { z?: number }).z)).toEqual([undefined, 3, 3]);
    });

    it('draws the RPO measured the same way, with no unrecovered marks', () => {
        const series = seriesOf(recoveryOption(ledger, 'rpo', period, 'tumult-dark'));
        expect(series.map((s) => s.name)).toEqual(['measured', 'manual']);
        expect(points(series[0])).toEqual([[manual.executed_at_ns / 1e6, 20]]);
        expect(series[0].markLine?.data).toEqual([{ yAxis: 60, name: 'target 60 s' }]);
    });

    it('spans the period and keeps the target and the highest point in view', () => {
        const option = recoveryOption(ledger, 'rto', period, 'tumult-dark') as {
            xAxis: { type: string; min: number; max: number };
            yAxis: { min: number; max: number };
        };
        expect(option.xAxis).toMatchObject({
            type: 'time',
            min: period.from_ns / 1e6,
            max: period.to_ns / 1e6,
            splitNumber: 4,
            axisLabel: { hideOverlap: true },
        });
        expect(option.yAxis).toMatchObject({ min: 0, max: 1000 });
        const low = recoveryOption(
            { ...ledger, tests: [met], rtoTarget: 10 },
            'rto',
            period,
            'tumult-dark',
        ) as { yAxis: { max: number } };
        expect(low.yAxis.max).toBe(20);
    });

    it('draws no rule when nothing sets a target', () => {
        const group = { ...ledger, rtoTarget: null };
        const [line] = seriesOf(recoveryOption(group, 'rto', period, 'tumult-dark'));
        expect(line.markLine).toBeUndefined();
    });

    it('follows the theme', () => {
        const light = seriesOf(recoveryOption(ledger, 'rto', period, 'tumult-light'));
        expect(light[2].itemStyle?.color).toBe(chartTokens('tumult-light').fail);
    });

    it('writes the tooltip of a point as text', () => {
        const evil = drTest({
            ...met,
            experiment_name: '<img src=x onerror="alert(1)">',
        });
        const [group] = recoveryGroups([evil], []);
        const option = recoveryOption(group, 'rto', period, 'tumult-dark') as {
            tooltip: { formatter: (p: unknown) => string };
        };
        const html = option.tooltip.formatter({ data: { id: evil.test_id } });
        expect(html).toContain('&lt;img src=x onerror=&quot;alert(1)&quot;&gt;');
        expect(html).not.toContain('<img');
        expect(html).toContain('RTO: <b>4 s</b>');
        expect(html).toContain('verdict met');
        expect(option.tooltip.formatter({ data: { id: 'nothing' } })).toBe('');
    });
});

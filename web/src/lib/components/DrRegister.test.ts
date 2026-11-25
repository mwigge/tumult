import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import DrRegister from './DrRegister.svelte';
import { DAY_MS, NOW_MS, badge, drTarget, ns, rowOf, textOf } from './dr-test-data';

const day = (ms: number) => {
    const d = new Date(ms);
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

const register = () =>
    render(DrRegister, {
        props: {
            targets: [
                drTarget({
                    system: 'orders',
                    state: 'ok',
                    last_test: {
                        id: 'automated:orders-dr',
                        origin: 'automated',
                        executed_at_ns: ns(NOW_MS - 10 * DAY_MS),
                        verdict: 'met',
                    },
                    next_due_at_ns: ns(NOW_MS + 355 * DAY_MS),
                    rto_measured_s: 4,
                    recovered: true,
                }),
                drTarget({
                    system: 'catalog',
                    state: 'due',
                    last_test: {
                        id: 'manual:01CAT',
                        origin: 'manual',
                        executed_at_ns: ns(NOW_MS - 350 * DAY_MS),
                        verdict: 'met',
                    },
                    next_due_at_ns: ns(NOW_MS + 15 * DAY_MS),
                    interval_source: 'function',
                    interval_days: 180,
                    rto_target_s: 900,
                    rto_measured_s: 300,
                    rpo_target_s: 60,
                    rpo_measured_s: 75,
                    recovered: true,
                }),
                drTarget({
                    system: 'payments-db',
                    state: 'overdue',
                    last_test: {
                        id: 'automated:payments-dr',
                        origin: 'automated',
                        executed_at_ns: ns(NOW_MS - 100 * DAY_MS),
                        verdict: 'not_recovered',
                    },
                    next_due_at_ns: ns(NOW_MS - 10 * DAY_MS),
                    interval_days: 90,
                    interval_source: 'plan',
                    plan: { id: 'plan-1', due_at_ns: ns(NOW_MS + 5 * DAY_MS) },
                    recovered: false,
                    rto_at_least_s: 30.1,
                }),
                drTarget({
                    system: 'archive',
                    environment: null,
                    state: 'never_tested',
                    rto_target_s: null,
                }),
            ],
        },
    }).body;

describe('the DR test register', () => {
    it('is a table with a caption, column headers and a row header per target', () => {
        const html = register();
        expect(html).toContain('<caption');
        expect(textOf(html)).toContain('Due register');
        for (const column of ['Target', 'State', 'Last test', 'Next due', 'Interval', 'Plan']) {
            expect(html).toContain(`<th scope="col">${column}</th>`);
        }
        expect(html.match(/<th scope="row"/g)).toHaveLength(4);
    });

    it('shows the state of each target in words, in the tone of the state', () => {
        const html = register();
        expect(rowOf(html, 'data-target="orders/prod"')).toMatch(badge('ok', 'ok'));
        expect(rowOf(html, 'data-target="catalog/prod"')).toMatch(badge('warn', 'due'));
        expect(rowOf(html, 'data-target="payments-db/prod"')).toMatch(badge('fail', 'overdue'));
        expect(rowOf(html, 'data-target="archive/-"')).toMatch(badge('neutral', 'never tested'));
    });

    it('links the last test to its run or to its manual record, with its date, origin and verdict', () => {
        const orders = rowOf(register(), 'data-target="orders/prod"');
        expect(orders).toContain('href="/experiments/orders-dr"');
        expect(textOf(orders)).toContain(`${day(NOW_MS - 10 * DAY_MS)} automated met`);
        const catalog = rowOf(register(), 'data-target="catalog/prod"');
        expect(catalog).toContain('href="/manual?tab=records&amp;record=01CAT"');
        expect(textOf(catalog)).toContain('manual met');
        expect(textOf(rowOf(register(), 'data-target="archive/-"'))).toContain('no DR test yet');
    });

    it('shows when the next test is due, the interval and where it comes from', () => {
        const html = register();
        expect(textOf(rowOf(html, 'data-target="catalog/prod"'))).toContain(
            `${day(NOW_MS + 15 * DAY_MS)} every 180 days from its function`,
        );
        expect(textOf(rowOf(html, 'data-target="payments-db/prod"'))).toContain(
            'every 90 days from its plan',
        );
        expect(textOf(rowOf(html, 'data-target="orders/prod"'))).toContain(
            'every 365 days by default',
        );
        expect(textOf(rowOf(html, 'data-target="archive/-"'))).toContain('test now');
    });

    it('links the plan that covers a target to the plans of the Manual page', () => {
        const row = rowOf(register(), 'data-target="payments-db/prod"');
        expect(row).toContain('href="/manual?tab=plans"');
        expect(textOf(row)).toContain(`due ${day(NOW_MS + 5 * DAY_MS)}`);
        expect(textOf(rowOf(register(), 'data-target="orders/prod"'))).toContain('no plan');
    });

    it('shows the RTO and RPO a target is held to and what its last test measured', () => {
        const html = register();
        expect(textOf(rowOf(html, 'data-target="orders/prod"'))).toContain('4 s target 10 s');
        const catalog = textOf(rowOf(html, 'data-target="catalog/prod"'));
        expect(catalog).toContain('5 min target 15 min');
        expect(catalog).toContain('75 s target 60 s');
        const payments = rowOf(html, 'data-target="payments-db/prod"');
        expect(textOf(payments)).toContain('at least 30.1 s, not recovered target 10 s');
        expect(payments).toMatch(/class="measured[^"]*\balarm"/);
        expect(rowOf(html, 'data-target="orders/prod"')).not.toMatch(/\balarm"/);
        expect(textOf(rowOf(html, 'data-target="archive/-"'))).toContain('not measured no target');
    });
});

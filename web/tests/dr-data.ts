// A DR view for the mocked tests of the Disaster recovery page: targets in
// every state of the register and 45 DR tests of the last year, automated
// and manual, and a stand-in for `GET /api/dr` that filters and pages them
// as the API does. The live test reads the real API.
import type { Page, Request } from '@playwright/test';

import type { DrState, DrTarget, DrTest, DrVerdict, DrView } from '../src/lib/types';

export const DAY_MS = 86_400_000;
const ns = (ms: number) => ms * 1e6;

/** A time `days` before `nowMs`, in unix nanoseconds. */
const ago = (nowMs: number, days: number) => ns(nowMs - days * DAY_MS);

function test(over: Partial<DrTest> & Pick<DrTest, 'test_id' | 'executed_at_ns'>): DrTest {
    const manual = over.test_id.startsWith('manual:');
    return {
        origin: manual ? 'manual' : 'automated',
        experiment_name: null,
        exercise_type: null,
        target_system: 'orders',
        target_environment: 'prod',
        outcome: manual ? 'passed' : 'completed',
        rto_target_s: 10,
        rto_measured_s: null,
        rto_at_least_s: null,
        recovered: true,
        no_outage: manual ? null : false,
        rpo_target_s: null,
        rpo_measured_s: null,
        data_verified: null,
        verdict: 'met',
        evidence_id: over.test_id.slice(over.test_id.indexOf(':') + 1),
        reviewer: null,
        plan_id: null,
        ...over,
    };
}

/** The 45 DR tests of the last year before `nowMs`, newest first. */
export function drTests(nowMs: number, name = 'orders failover'): DrTest[] {
    const orders = Array.from({ length: 36 }, (_, i) => {
        const measured = i === 7 ? 12.5 : 4 + ((i * 7) % 5) * 0.7;
        return test({
            test_id: `automated:orders-dr-${String(i).padStart(2, '0')}`,
            experiment_name: name,
            executed_at_ns: ago(nowMs, 10 + i * 9),
            outcome: measured > 10 ? 'deviated' : 'completed',
            rto_measured_s: measured,
            verdict: measured > 10 ? 'breached' : 'met',
        });
    });
    const ledger = { target_system: 'ledger', rto_target_s: 60, rpo_target_s: 30 };
    const others = [
        test({
            ...ledger,
            test_id: 'automated:ledger-dr-5',
            experiment_name: 'ledger failover',
            executed_at_ns: ago(nowMs, 5),
            rto_measured_s: 38,
            rpo_measured_s: 8,
        }),
        test({
            ...ledger,
            test_id: 'manual:01LEDGERDRILL',
            experiment_name: 'ledger failover drill',
            exercise_type: 'drill',
            executed_at_ns: ago(nowMs, 60),
            rto_measured_s: 50,
            rpo_measured_s: 12,
            data_verified: true,
            reviewer: 'bob',
        }),
        test({
            ...ledger,
            test_id: 'automated:ledger-dr-120',
            experiment_name: 'ledger failover',
            executed_at_ns: ago(nowMs, 120),
            outcome: 'deviated',
            recovered: false,
            rto_at_least_s: 75,
            verdict: 'not_recovered',
        }),
        test({
            ...ledger,
            test_id: 'automated:ledger-dr-200',
            experiment_name: 'ledger failover',
            executed_at_ns: ago(nowMs, 200),
            rto_measured_s: 42,
            rpo_measured_s: 9,
        }),
        test({
            ...ledger,
            test_id: 'manual:01LEDGERFAILOVER',
            experiment_name: 'ledger failover by hand',
            exercise_type: 'failover',
            executed_at_ns: ago(nowMs, 300),
            rto_measured_s: 55,
            rpo_measured_s: 20,
            data_verified: true,
            reviewer: 'carol',
        }),
        test({
            test_id: 'automated:billing-dr',
            experiment_name: 'billing failover',
            target_system: 'billing',
            executed_at_ns: ago(nowMs, 200),
            outcome: 'deviated',
            rto_measured_s: 14,
            verdict: 'breached',
        }),
        test({
            test_id: 'automated:payments-dr',
            experiment_name: 'payments-db failover',
            target_system: 'payments-db',
            executed_at_ns: ago(nowMs, 100),
            outcome: 'deviated',
            recovered: false,
            rto_at_least_s: 30.1,
            verdict: 'not_recovered',
        }),
        test({
            test_id: 'automated:search-dr',
            experiment_name: 'search zone failover',
            target_system: 'search',
            target_environment: 'staging',
            executed_at_ns: ago(nowMs, 5),
            recovered: null,
            no_outage: true,
            verdict: 'not_measured',
        }),
        test({
            test_id: 'manual:01CATALOGRESTORE',
            experiment_name: 'catalog restore',
            exercise_type: 'restore',
            target_system: 'catalog',
            executed_at_ns: ago(nowMs, 350),
            rto_target_s: 900,
            rto_measured_s: 300,
            rpo_target_s: 60,
            rpo_measured_s: 10,
            data_verified: true,
            reviewer: 'bob',
        }),
    ];
    return [...orders, ...others].sort(
        (a, b) => b.executed_at_ns - a.executed_at_ns || a.test_id.localeCompare(b.test_id),
    );
}

function target(over: Partial<DrTarget> & Pick<DrTarget, 'system' | 'state'>): DrTarget {
    return {
        environment: 'prod',
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

const last = (id: string, at: number, verdict: DrVerdict = 'met') => ({
    id,
    origin: id.startsWith('manual:') ? ('manual' as const) : ('automated' as const),
    executed_at_ns: at,
    verdict,
});

/** The register of the targets of [`drTests`], in every state, by target. */
export function drTargets(nowMs: number): DrTarget[] {
    return [
        target({
            system: 'archive',
            state: 'never_tested',
            interval_source: 'plan',
            plan: { id: 'plan-archive', due_at_ns: ago(nowMs, -20) },
            rto_target_s: 120,
            rpo_target_s: 30,
        }),
        target({
            system: 'billing',
            state: 'overdue',
            last_test: last('automated:billing-dr', ago(nowMs, 200), 'breached'),
            next_due_at_ns: ago(nowMs, 20),
            interval_days: 180,
            interval_source: 'function',
            rto_measured_s: 14,
            recovered: true,
        }),
        target({
            system: 'catalog',
            state: 'due',
            last_test: last('manual:01CATALOGRESTORE', ago(nowMs, 350)),
            next_due_at_ns: ago(nowMs, -15),
            rto_target_s: 900,
            rpo_target_s: 60,
            rto_measured_s: 300,
            rpo_measured_s: 10,
            recovered: true,
        }),
        target({
            system: 'ledger',
            state: 'ok',
            last_test: last('automated:ledger-dr-5', ago(nowMs, 5)),
            next_due_at_ns: ago(nowMs, -360),
            rto_target_s: 60,
            rpo_target_s: 30,
            rto_measured_s: 38,
            rpo_measured_s: 8,
            recovered: true,
        }),
        target({
            system: 'orders',
            state: 'ok',
            last_test: last('automated:orders-dr-00', ago(nowMs, 10)),
            next_due_at_ns: ago(nowMs, -355),
            rto_measured_s: 4,
            recovered: true,
        }),
        target({
            system: 'payments-db',
            state: 'overdue',
            last_test: last('automated:payments-dr', ago(nowMs, 100), 'not_recovered'),
            next_due_at_ns: ago(nowMs, 10),
            interval_days: 90,
            interval_source: 'plan',
            plan: { id: 'plan-payments', due_at_ns: ago(nowMs, -5) },
            recovered: false,
            rto_at_least_s: 30.1,
        }),
        target({ system: 'search', environment: 'staging', state: 'never_tested' }),
    ];
}

/** A lake of DR tests and targets that the stand-in answers from. */
export interface DrLake {
    targets: DrTarget[];
    tests: DrTest[];
}

export const fullLake = (nowMs: number): DrLake => ({
    targets: drTargets(nowMs),
    tests: drTests(nowMs),
});

export const emptyLake = (): DrLake => ({ targets: [], tests: [] });

const VERDICTS: DrVerdict[] = ['met', 'breached', 'not_recovered', 'not_measured'];
const STATES: DrState[] = ['ok', 'due', 'overdue', 'never_tested'];

function counts(tests: DrTest[], origin: DrTest['origin']): Record<DrVerdict, number> {
    const mine = tests.filter((t) => t.origin === origin);
    return Object.fromEntries(
        VERDICTS.map((v) => [v, mine.filter((t) => t.verdict === v).length]),
    ) as Record<DrVerdict, number>;
}

/** What `GET /api/dr` answers from `lake` for the query of `url`. The cursor is an offset. */
export function answerOf(lake: DrLake, url: URL, nowMs: number): DrView {
    const q = url.searchParams;
    const target = q.get('target') ?? '';
    const environment = q.get('environment') ?? '';
    const toNs = Number(q.get('to') ?? ns(nowMs));
    const fromNs = Number(q.get('from') ?? toNs - ns(365 * DAY_MS));
    const limit = Math.min(Math.max(Number(q.get('limit') ?? 100), 1), 1000);
    const offset = Number(q.get('cursor') ?? 0);
    const kept = (system: string | null, env: string | null) =>
        (!target || system === target) && (!environment || env === environment);
    const targets = lake.targets.filter((t) => kept(t.system, t.environment));
    const tests = lake.tests.filter(
        (t) =>
            kept(t.target_system, t.target_environment) &&
            t.executed_at_ns >= fromNs &&
            t.executed_at_ns <= toNs,
    );
    const page = tests.slice(offset, offset + limit);
    const truncated = offset + limit < tests.length;
    return {
        as_of_ns: ns(nowMs),
        from_ns: fromNs,
        to_ns: toNs,
        targets,
        tests: page,
        count: page.length,
        total: tests.length,
        truncated,
        next_cursor: truncated ? String(offset + limit) : null,
        summary: {
            states: Object.fromEntries(
                STATES.map((s) => [s, targets.filter((t) => t.state === s).length]),
            ) as Record<DrState, number>,
            tests: {
                total: tests.length,
                automated: counts(tests, 'automated'),
                manual: counts(tests, 'manual'),
            },
        },
    };
}

const ME = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

/**
 * Answer the API of the page from `lake`: `GET /api/dr` as [`answerOf`]
 * says, `status` for it when given, and `GET /api/me`. Returns the URLs of
 * the DR requests, in order.
 */
export async function serveDr(
    page: Page,
    lake: DrLake,
    nowMs: number,
    status = 200,
): Promise<URL[]> {
    const seen: URL[] = [];
    await page.route('**/api/**', async (route) => {
        const request: Request = route.request();
        const url = new URL(request.url());
        let body: unknown = {};
        let code = 200;
        if (url.pathname === '/api/me') body = ME;
        else if (url.pathname === '/api/dr') {
            seen.push(url);
            code = status;
            body = status === 200 ? answerOf(lake, url, nowMs) : { error: 'the lake is busy' };
        }
        await route.fulfill({ status: code, json: body });
    });
    return seen;
}

import { expect, test, type Page } from '@playwright/test';

import { installChartHooks } from './charts';

// The API is mocked. The distribution chart renders to canvas, so it is
// asserted through its accessible name, the option mirrored at
// window.__tumultCharts and the evidence table next to it.

type Json = Record<string, unknown>;
type Reply = { status: number; body: unknown };

const openMe = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};
const session = (role: string) => ({
    auth_required: true,
    authenticated: true,
    username: role,
    role,
    env_scopes: [],
});

const targets = {
    targets: [
        {
            target: 'checkout',
            environments: ['demo', 'prod', null],
            runs: 4,
            last_run_ns: 1_790_000_000_000_000_000,
        },
        { target: 'payments', environments: ['demo'], runs: 2, last_run_ns: 1 },
    ],
    range: '30d',
    as_of_ns: 1_790_000_000_000_000_000,
};

const distribution = (from: number, to: number) =>
    Array.from({ length: 21 }, (_, i) => ({
        p: i * 5,
        value: Math.round((from + ((to - from) * i) / 20) * 10) / 10,
    }));

const side = (version: string, p50: number, p95: number, p99: number, over: Json = {}) => ({
    version,
    runs: [`run-${version}`],
    samples: 24,
    dropped: 0,
    p50,
    p95,
    p99,
    attribution: { declared: 0, observed: 1, inferred: 0 },
    run_details: [
        {
            experiment_id: `run-${version}`,
            attribution: 'observed',
            status: 'completed',
            truncated: false,
            aborted: false,
            included: true,
        },
    ],
    partial: { truncated: 0, aborted: 0, halted: 0, deviated: 0, excluded: 0 },
    windows: {
        planned: 24,
        completed: 24,
        unanswered: 0,
        lost: 0,
        unanswered_share: 0,
        censored: 0,
    },
    requests: { sent: 2400, failed: 0, failure_share: 0, runs_without_counts: 0 },
    distribution: distribution(p50 - 3, p99 + 2),
    ...over,
});

const stored = (over: Json = {}) => ({
    target: 'checkout',
    probe: 'load.latency_p95_ms',
    baseline_version: '0.9.0',
    candidate_version: '1.0.0',
    quantile: 'p95',
    baseline_n: 24,
    candidate_n: 24,
    baseline_p50: 8,
    baseline_p95: 9.1,
    baseline_p99: 9.9,
    candidate_p50: 8.1,
    candidate_p95: 9.3,
    candidate_p99: 10,
    delta: 0.2,
    delta_pct: 2.2,
    ci_low: -0.1,
    ci_high: 0.4,
    confidence: 0.95,
    margin: 0.5,
    verdict: 'unchanged',
    reasons: [],
    computed_at_ns: 1_789_000_000_000_000_000,
    envs: ['demo'],
    ...over,
});

const comparison = (over: Json = {}) => ({
    quantile: 'p95',
    delta: 38.2,
    delta_pct: 412,
    ci_low: 36.9,
    ci_high: 39.4,
    confidence: 0.95,
    margin: 0.5,
    iterations: 2000,
    seed: 7,
    min_samples: 20,
    min_runs: 3,
    low_run_count: true,
    failure_share: { baseline: 0, candidate: 0, delta: 0, margin: 0.01 },
    ...over,
});

const answer = (over: Json = {}) => ({
    target: 'checkout',
    probe: 'load.latency_p95_ms',
    environment: 'demo',
    environments_seen: ['demo', 'prod'],
    range: '30d',
    since_ns: 1_787_000_000_000_000_000,
    selection: 'latest_deployment',
    selection_rule: 'in demo, 1.1.0 was deployed last and 1.0.0 before it',
    backfill: 'done',
    include_partial: false,
    verdict: 'regressed',
    reason: 'latency',
    reason_codes: ['latency'],
    reasons: ['p95 difference interval lies above the margin'],
    baseline: side('1.0.0', 8.1, 9.3, 10),
    candidate: side('1.1.0', 41, 47.5, 49),
    comparison: comparison(),
    versions_seen: [
        {
            version: '1.1.0',
            first_seen_ns: 1,
            last_seen_ns: 2,
            last_deployed_ns: 1,
            redeployed: false,
        },
        {
            version: '1.0.0',
            first_seen_ns: 0,
            last_seen_ns: 2,
            last_deployed_ns: 0,
            redeployed: false,
        },
    ],
    unattributed_runs: {
        count: 1,
        reasons: { no_target_spans_near_run: 1 },
        runs: [{ experiment_id: 'abc', environment: 'demo', reason: 'no_target_spans_near_run' }],
    },
    runs_on_other_versions: 2,
    stored: false,
    duplicate: false,
    history: [stored()],
    as_of_ns: 1_790_000_000_000_000_000,
    ...over,
});

interface Mock {
    me?: unknown;
    targets?: () => Reply;
    read?: (url: URL) => Reply;
    store?: (url: URL) => Reply;
}

interface Calls {
    reads: URL[];
    stores: URL[];
}

async function mockApi(page: Page, mock: Mock = {}): Promise<Calls> {
    const calls: Calls = { reads: [], stores: [] };
    await page.route('**/api/**', async (route) => {
        const req = route.request();
        const url = new URL(req.url());
        const path = url.pathname;
        let reply: Reply = { status: 200, body: {} };
        if (path === '/api/me') reply = { status: 200, body: mock.me ?? openMe };
        else if (path === '/api/perf') reply = mock.targets?.() ?? { status: 200, body: targets };
        else if (path.startsWith('/api/perf/') && path.endsWith('/verdicts')) {
            if (req.method() === 'POST') calls.stores.push(url);
            reply = mock.store?.(url) ?? {
                status: 201,
                body: answer({ stored: true, duplicate: false }),
            };
        } else if (path.startsWith('/api/perf/')) {
            calls.reads.push(url);
            reply = mock.read?.(url) ?? { status: 200, body: answer() };
        }
        await route.fulfill({
            status: reply.status,
            contentType: 'application/json',
            body: JSON.stringify(reply.body),
        });
    });
    return calls;
}

const lastOf = (urls: URL[]): URL => {
    const last = urls.at(-1);
    if (!last) throw new Error('no request was recorded');
    return last;
};

test('GIVEN a regressed candidate WHEN the page opens THEN the verdict, the delta and the evidence are shown', async ({
    page,
}) => {
    await installChartHooks(page);
    const calls = await mockApi(page);
    await page.goto('/performance');

    await expect(page.getByRole('heading', { name: 'Performance', level: 1 })).toBeVisible();
    await expect(page.getByLabel('Target')).toHaveValue('checkout');

    const verdict = page.getByTestId('verdict');
    await expect(verdict.getByTestId('verdict-badge')).toHaveText('regressed');
    await expect(verdict).toContainText('p95 difference interval lies above the margin');
    await expect(verdict.getByTestId('few-runs')).toHaveText('fewer than 5 runs');
    await expect(page.getByTestId('low-run-count')).toContainText('Fewer than 5 runs on a side');
    await expect(verdict).toContainText('deployment order');
    await expect(verdict).toContainText('environment demo');
    await expect(verdict.getByTestId('attribution-baseline')).toContainText(
        '1 observed, 0 declared, 0 inferred',
    );
    await expect(verdict.getByTestId('attribution-candidate')).toContainText(
        '1 observed, 0 declared, 0 inferred',
    );
    await expect(verdict).not.toContainText('span timing');

    const delta = page.getByTestId('delta');
    await expect(delta).toContainText('+38.2 ms');
    await expect(delta).toContainText('+412.0 %');
    await expect(delta).toContainText('95 % confidence interval [36.9, 39.4] ms');
    await expect(delta).toContainText('noise margin 0.5 ms');

    await expect(
        page.getByRole('img', {
            name: 'Latency distribution: baseline 1.0.0 p95 9.3 ms versus candidate 1.1.0 p95 47.5 ms',
        }),
    ).toBeVisible();
    const chart = await page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.perf as
            { series?: { name?: string; data?: unknown[]; markLine?: unknown }[] } | undefined;
        return {
            names: opt?.series?.map((s) => s.name) ?? [],
            points: opt?.series?.map((s) => s.data?.length ?? 0) ?? [],
            marked: Boolean(opt?.series?.some((s) => s.markLine)),
        };
    });
    expect(chart).toEqual({
        names: ['baseline 1.0.0', 'candidate 1.1.0'],
        points: [21, 21],
        marked: true,
    });

    const evidence = page.getByRole('table', { name: 'Latency by version' });
    await expect(evidence.locator('tbody tr')).toHaveCount(2);
    await expect(evidence.locator('tbody tr').nth(0).locator('th, td')).toHaveText([
        'baseline',
        '1.0.0',
        '1',
        '24',
        '0',
        '8.1',
        '9.3',
        '10.0',
    ]);
    await expect(evidence.locator('tbody tr').nth(1)).toContainText('1.1.0');
    await expect(evidence.locator('tbody tr').nth(1)).toContainText('47.5');

    const unattributed = page.getByTestId('unattributed');
    await expect(unattributed).toContainText('1 run could not be attributed');
    await expect(unattributed).toContainText('no_target_spans_near_run: 1');
    await expect(unattributed).toContainText('2 runs on other versions');

    const history = page.getByRole('table', { name: 'Stored verdicts' });
    await expect(history.locator('tbody tr')).toHaveCount(1);
    await expect(history.locator('tbody tr').first()).toContainText('0.9.0 → 1.0.0');
    await expect(history.locator('tbody tr').first()).toContainText('unchanged');
    await expect(history.locator('tbody tr').first()).toContainText('[-0.1, 0.4]');
    await expect(history.locator('tbody tr').first()).toContainText('demo');

    // The default read names the quantile and the range; empty parameters are omitted.
    expect(lastOf(calls.reads).pathname).toBe('/api/perf/checkout');
    expect(lastOf(calls.reads).search).toBe('?quantile=p95&range=30d');
});

test('GIVEN an unchanged candidate with inferred attribution WHEN the page opens THEN it says unchanged and flags the weaker evidence', async ({
    page,
}) => {
    await mockApi(page, {
        read: () => ({
            status: 200,
            body: answer({
                verdict: 'unchanged',
                reasons: ['p95 difference interval lies within the margin'],
                selection: 'explicit',
                environment: null,
                candidate: side('1.1.0', 8.2, 9.4, 10.1, {
                    attribution: { declared: 0, observed: 0, inferred: 1 },
                }),
                comparison: comparison({
                    low_run_count: false,
                    delta: 0.1,
                    delta_pct: 1.1,
                    ci_low: -0.2,
                    ci_high: 0.3,
                }),
                unattributed_runs: { count: 0, reasons: {}, runs: [] },
                runs_on_other_versions: 0,
                history: [],
            }),
        }),
    });
    await page.goto('/performance');

    const verdict = page.getByTestId('verdict');
    await expect(verdict.getByTestId('verdict-badge')).toHaveText('unchanged');
    await expect(verdict).toContainText('p95 difference interval lies within the margin');
    await expect(verdict).toContainText('explicit versions');
    await expect(verdict).toContainText('environment untagged');
    await expect(verdict.getByTestId('attribution-candidate')).toContainText(
        '0 observed, 0 declared, 1 inferred',
    );
    await expect(verdict).toContainText(
        'Inferred attribution comes from span timing and is weaker evidence',
    );
    await expect(page.getByTestId('delta')).toContainText('+0.1 ms');
    await expect(page.getByTestId('unattributed')).toHaveCount(0);
    await expect(page.getByText('No verdict has been stored for this target yet.')).toBeVisible();
});

test('GIVEN unanswered windows on the candidate WHEN the page opens THEN it says inconclusive and shows the partial counts per side', async ({
    page,
}) => {
    const reason =
        'candidate 1.2.0: 6 of 24 windows went unanswered (25 %); the sides are not comparable';
    await mockApi(page, {
        read: () => ({
            status: 200,
            body: answer({
                verdict: 'inconclusive',
                reasons: [reason],
                baseline: side('1.1.0', 41, 47.5, 49),
                candidate: side('1.2.0', 40, 46, 48, {
                    samples: 18,
                    partial: { truncated: 1, aborted: 0, halted: 2, deviated: 3, excluded: 1 },
                    windows: {
                        planned: 24,
                        completed: 18,
                        unanswered: 6,
                        lost: 2,
                        unanswered_share: 0.25,
                        censored: 4,
                    },
                }),
                comparison: comparison({
                    delta: null,
                    delta_pct: null,
                    ci_low: null,
                    ci_high: null,
                }),
            }),
        }),
    });
    await page.goto('/performance');

    const verdict = page.getByTestId('verdict');
    await expect(verdict.getByTestId('verdict-badge')).toHaveText('inconclusive');
    await expect(verdict).toContainText(reason);
    await expect(page.getByTestId('delta')).toContainText('No difference was computed');

    const partial = page.getByRole('table', { name: 'Partial runs and windows by version' });
    await expect(partial.locator('tbody tr').nth(0).locator('th, td')).toHaveText([
        'baseline',
        '1.1.0',
        '0',
        '0',
        '0',
        '0',
        '0',
        '24',
        '24',
        '0 (0.0 %)',
        '0',
        '0',
    ]);
    await expect(partial.locator('tbody tr').nth(1).locator('th, td')).toHaveText([
        'candidate',
        '1.2.0',
        '1',
        '0',
        '2',
        '3',
        '1',
        '24',
        '18',
        '6 (25.0 %)',
        '2',
        '4',
    ]);
});

test('GIVEN a candidate that failed a third of its requests WHEN the page opens THEN the requests, the censored windows, the runs and the rule behind the pair are shown', async ({
    page,
}) => {
    const rule = 'in demo, 1.1.0 was deployed last (again, after 1.2.0) and 1.2.0 before it';
    const run = (id: string, over: Json = {}) => ({
        experiment_id: id,
        attribution: 'declared',
        status: 'completed',
        truncated: false,
        aborted: false,
        included: true,
        ...over,
    });
    await mockApi(page, {
        read: () => ({
            status: 200,
            body: answer({
                reason: 'failure_share',
                reason_codes: ['failure_share', 'failure_data_missing'],
                reasons: ['candidate 1.1.0: 420 of 1200 requests failed (35.0%)'],
                selection_rule: rule,
                backfill: 'running',
                baseline: side('1.2.0', 41, 47.5, 49, {
                    requests: {
                        sent: 0,
                        failed: 0,
                        failure_share: null,
                        runs_without_counts: 1,
                    },
                }),
                candidate: side('1.1.0', 8, 9, 10, {
                    runs: ['run-a'],
                    dropped: 2,
                    run_details: [
                        run('run-a'),
                        run('run-b', { status: 'deviated', truncated: true, included: false }),
                        run('run-c', { status: 'halted', aborted: true, included: false }),
                    ],
                    windows: {
                        planned: 48,
                        completed: 40,
                        unanswered: 0,
                        lost: 3,
                        unanswered_share: 0,
                        censored: 5,
                    },
                    requests: {
                        sent: 1200,
                        failed: 420,
                        failure_share: 0.35,
                        runs_without_counts: 0,
                    },
                }),
                comparison: comparison({
                    failure_share: { baseline: 0.004, candidate: 0.35, delta: 0.346, margin: 0.01 },
                }),
                versions_seen: [
                    {
                        version: '1.1.0',
                        first_seen_ns: 1,
                        last_seen_ns: 9,
                        last_deployed_ns: 7,
                        redeployed: true,
                    },
                    {
                        version: '1.2.0',
                        first_seen_ns: 4,
                        last_seen_ns: 6,
                        last_deployed_ns: 4,
                        redeployed: false,
                    },
                ],
            }),
        }),
    });
    await page.goto('/performance');

    const verdict = page.getByTestId('verdict');
    await expect(verdict.getByTestId('selection-rule')).toHaveText(rule);
    await expect(verdict.getByTestId('backfill')).toContainText('still being read back');
    await expect(verdict.getByTestId('redeployed')).toHaveText(
        '1.1.0 was deployed again after a later version; its latest deployment counts.',
    );

    const latency = page.getByRole('table', { name: 'Latency by version' });
    await expect(latency.locator('tbody tr').nth(1).locator('th, td')).toHaveText([
        'candidate',
        '1.1.0',
        '1',
        '24',
        '2',
        '8.0',
        '9.0',
        '10.0',
    ]);

    const windows = page.getByRole('table', { name: 'Partial runs and windows by version' });
    await expect(windows.locator('tbody tr').nth(1).locator('th, td')).toHaveText([
        'candidate',
        '1.1.0',
        '0',
        '0',
        '0',
        '0',
        '0',
        '48',
        '40',
        '0 (0.0 %)',
        '3',
        '5',
    ]);

    const requests = page.getByRole('table', { name: 'Requests by version' });
    await expect(requests.locator('tbody tr').nth(0).locator('th, td')).toHaveText([
        'baseline',
        '1.2.0',
        '0',
        '0',
        '–',
        '1',
    ]);
    await expect(requests.locator('tbody tr').nth(1).locator('th, td')).toHaveText([
        'candidate',
        '1.1.0',
        '1200',
        '420',
        '35.0 %',
        '0',
    ]);
    await expect(page.getByTestId('failure-share')).toHaveText(
        'Failure share, candidate minus baseline: +34.6 percentage points (margin 1.0).',
    );

    const runs = page.getByRole('table', { name: 'Runs by version' });
    await expect(runs.locator('tbody tr')).toHaveCount(4);
    await expect(runs.locator('tbody tr').nth(0).locator('th, td')).toHaveText([
        'baseline',
        'run-1.2.0',
        'observed',
        'completed',
        'whole',
        'yes',
    ]);
    await expect(runs.locator('tbody tr').nth(2).locator('th, td')).toHaveText([
        'candidate',
        'run-b',
        'declared',
        'deviated',
        'truncated',
        'left out',
    ]);
    await expect(runs.locator('tbody tr').nth(3).locator('th, td')).toHaveText([
        'candidate',
        'run-c',
        'declared',
        'halted',
        'aborted',
        'left out',
    ]);
});

test('GIVEN a version and a target that are markup WHEN the page renders and the chart tooltip opens THEN they are shown as text and nothing runs', async ({
    page,
}) => {
    const hostile = '<img src=x onerror="window.__pwned=1">';
    await mockApi(page, {
        targets: () => ({
            status: 200,
            body: {
                ...targets,
                targets: [{ target: hostile, environments: [hostile], runs: 1, last_run_ns: 1 }],
            },
        }),
        read: () => ({
            status: 200,
            body: answer({
                target: hostile,
                environment: hostile,
                environments_seen: [hostile],
                reasons: [`candidate ${hostile}: slower`],
                selection_rule: `${hostile} was deployed last`,
                candidate: side(hostile, 41, 47.5, 49),
                versions_seen: [
                    {
                        version: hostile,
                        first_seen_ns: 1,
                        last_seen_ns: 2,
                        last_deployed_ns: 1,
                        redeployed: true,
                    },
                ],
                unattributed_runs: {
                    count: 1,
                    reasons: { [hostile]: 1 },
                    runs: [{ experiment_id: hostile, environment: hostile, reason: hostile }],
                },
                history: [stored({ candidate_version: hostile, envs: [hostile] })],
            }),
        }),
    });
    await page.goto('/performance');

    const verdict = page.getByTestId('verdict');
    await expect(verdict).toContainText(`candidate ${hostile}: slower`);
    await expect(verdict.getByTestId('selection-rule')).toHaveText(`${hostile} was deployed last`);
    await expect(verdict.getByTestId('redeployed')).toContainText(hostile);
    await expect(page.getByRole('table', { name: 'Runs by version' })).toContainText(
        `run-${hostile}`,
    );

    // The tooltip is the one place a chart builds HTML.
    const canvas = page.locator('[data-chart="perf"] canvas');
    await canvas.hover({ position: { x: 260, y: 150 } });
    await canvas.hover({ position: { x: 300, y: 140 } });
    const tip = page.locator('.perf-tooltip');
    await expect(tip).toBeVisible();
    await expect(tip).toContainText(`candidate ${hostile}`);
    await expect(tip).toContainText('baseline 1.0.0');

    await expect(page.locator('img')).toHaveCount(0);
    const ran = await page.evaluate(() => '__pwned' in window);
    expect(ran).toBe(false);
});

test('GIVEN an operator WHEN storing the verdict THEN the POST carries the same query and the outcome is announced', async ({
    page,
}) => {
    let storedOnce = false;
    const calls = await mockApi(page, {
        me: session('operator'),
        read: () => ({
            status: 200,
            body: answer({
                history: storedOnce
                    ? [
                          stored({
                              baseline_version: '1.0.0',
                              candidate_version: '1.1.0',
                              verdict: 'regressed',
                              computed_at_ns: 1_790_000_000_000_000_000,
                          }),
                          stored(),
                      ]
                    : [stored()],
            }),
        }),
        store: () => {
            const duplicate = storedOnce;
            storedOnce = true;
            return {
                status: duplicate ? 200 : 201,
                body: answer({ stored: true, duplicate }),
            };
        },
    });
    await page.goto('/performance');
    await expect(page.getByTestId('verdict-badge')).toHaveText('regressed');

    const query = '?quantile=p99&env=demo&baseline=1.0.0&candidate=1.1.0&range=30d&include=partial';
    await page.getByLabel('Environment').selectOption('demo');
    await page.getByLabel('Quantile').selectOption('p99');
    await page.getByLabel('Baseline version').selectOption('1.0.0');
    await page.getByLabel('Candidate version').selectOption('1.1.0');
    await page.getByLabel('Include partial runs').check();
    await expect.poll(() => lastOf(calls.reads).search).toBe(query);
    await expect(page).toHaveURL(/\/performance\?target=checkout&env=demo$/);

    const history = page.getByRole('table', { name: 'Stored verdicts' });
    await expect(history.locator('tbody tr')).toHaveCount(1);

    await page.getByRole('button', { name: 'Store verdict' }).click();
    const status = page.getByTestId('store-status');
    await expect(status).toHaveText('Verdict stored');
    await expect(status).toHaveAttribute('aria-live', 'polite');
    expect(calls.stores).toHaveLength(1);
    expect(lastOf(calls.stores).pathname).toBe('/api/perf/checkout/verdicts');
    expect(lastOf(calls.stores).search).toBe(query);
    // The history is read again after the store.
    await expect(history.locator('tbody tr')).toHaveCount(2);
    await expect(history.locator('tbody tr').first()).toContainText('1.0.0 → 1.1.0');

    await page.getByRole('button', { name: 'Store verdict' }).click();
    await expect(status).toHaveText('Already stored — nothing appended');
    expect(calls.stores).toHaveLength(2);
    await expect(history.locator('tbody tr')).toHaveCount(2);
});

test('GIVEN a viewer WHEN the page opens THEN the store action is not available', async ({
    page,
}) => {
    const calls = await mockApi(page, { me: session('viewer') });
    await page.goto('/performance');
    await expect(page.getByTestId('verdict-badge')).toHaveText('regressed');

    await expect(page.getByRole('button', { name: 'Store verdict' })).toBeDisabled();
    await expect(page.getByText('Storing a verdict needs the operator role.')).toBeVisible();
    expect(calls.stores).toHaveLength(0);
});

test('GIVEN the server refuses the store with 403 WHEN storing THEN its message is shown as an error and nothing is reported as stored', async ({
    page,
}) => {
    const calls = await mockApi(page, {
        me: session('operator'),
        store: () => ({ status: 403, body: { error: 'operator role required' } }),
    });
    await page.goto('/performance');

    await page.getByRole('button', { name: 'Store verdict' }).click();
    const failure = page.getByTestId('store-error');
    await expect(failure).toContainText('operator role required');
    await expect(failure).toHaveAttribute('role', 'alert');
    expect(calls.stores).toHaveLength(1);
    await expect(page.getByTestId('store-status')).toHaveText('');
    await expect(page.getByText('Verdict stored')).toHaveCount(0);
    await expect(
        page.getByRole('table', { name: 'Stored verdicts' }).locator('tbody tr'),
    ).toHaveCount(1);
    // The evidence stays on screen: a refused store is not a failed read.
    await expect(page.getByTestId('verdict-badge')).toHaveText('regressed');
});

test('GIVEN the read fails with 500 WHEN the page opens THEN the error state offers a retry and is not the empty state', async ({
    page,
}) => {
    let failing = true;
    await mockApi(page, {
        read: () =>
            failing
                ? { status: 500, body: { error: 'lake query failed' } }
                : { status: 200, body: answer() },
    });
    await page.goto('/performance');

    const alert = page.getByRole('alert');
    await expect(alert).toContainText('Could not load the performance evidence');
    await expect(alert).toContainText('lake query failed');
    await expect(page.getByText('No load-tested targets yet')).toHaveCount(0);
    await expect(page.getByTestId('no-evidence')).toHaveCount(0);
    await expect(page.getByTestId('verdict')).toHaveCount(0);

    failing = false;
    await alert.getByRole('button', { name: 'Retry' }).click();
    await expect(page.getByTestId('verdict-badge')).toHaveText('regressed');
    await expect(page.getByRole('alert')).toHaveCount(0);
});

test('GIVEN the target list fails with 500 WHEN the page opens THEN the error state is shown instead of the empty state', async ({
    page,
}) => {
    await mockApi(page, { targets: () => ({ status: 500, body: { error: 'lake unavailable' } }) });
    await page.goto('/performance');

    const alert = page.getByRole('alert');
    await expect(alert).toContainText('lake unavailable');
    await expect(alert.getByRole('button', { name: 'Retry' })).toBeVisible();
    await expect(page.getByText('No load-tested targets yet')).toHaveCount(0);
});

test('GIVEN no load-tested targets WHEN the page opens THEN the empty state explains how evidence gets there', async ({
    page,
}) => {
    const calls = await mockApi(page, {
        targets: () => ({ status: 200, body: { targets: [], range: '30d', as_of_ns: 1 } }),
    });
    await page.goto('/performance');

    const empty = page.getByTestId('empty');
    await expect(empty).toContainText('No load-tested targets yet');
    await expect(empty).toContainText('tumult run');
    await expect(empty).toContainText('service.version');
    await expect(page.getByRole('alert')).toHaveCount(0);
    expect(calls.reads).toHaveLength(0);
});

test('GIVEN a target without a service.version WHEN the read answers 404 THEN the server reason is shown as no evidence, not as a failure', async ({
    page,
}) => {
    await mockApi(page, {
        read: () => ({
            status: 404,
            body: { error: 'target checkout reports no service.version in the last 30d' },
        }),
    });
    await page.goto('/performance');

    const none = page.getByTestId('no-evidence');
    await expect(none).toContainText('No performance evidence');
    await expect(none).toContainText('target checkout reports no service.version in the last 30d');
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(page.getByText('No load-tested targets yet')).toHaveCount(0);
});

test('GIVEN evidence in several environments WHEN the read answers 400 THEN the message is shown and the environment picker resolves it', async ({
    page,
}) => {
    await mockApi(page, {
        read: (url) =>
            url.searchParams.get('env')
                ? { status: 200, body: answer({ environment: url.searchParams.get('env') }) }
                : {
                      status: 400,
                      body: {
                          error: 'evidence in several environments (demo, prod); pass env or explicit versions',
                      },
                  },
    });
    await page.goto('/performance');

    await expect(page.getByTestId('needs-input')).toContainText(
        'evidence in several environments (demo, prod); pass env or explicit versions',
    );
    await expect(page.getByRole('alert')).toHaveCount(0);

    const env = page.getByLabel('Environment');
    await expect(env).toBeEnabled();
    await expect(env.locator('option')).toHaveText(['auto', 'demo', 'prod', 'untagged']);
    await env.selectOption('prod');
    await expect(page.getByTestId('verdict')).toContainText('environment prod');
    await expect(page.getByTestId('needs-input')).toHaveCount(0);
    await expect(page).toHaveURL(/\/performance\?target=checkout&env=prod$/);
});

test('GIVEN target and env in the URL WHEN the page opens THEN the pickers and the read follow them', async ({
    page,
}) => {
    const calls = await mockApi(page);
    await page.goto('/performance?target=payments&env=demo');

    await expect(page.getByLabel('Target')).toHaveValue('payments');
    await expect(page.getByLabel('Environment')).toHaveValue('demo');
    await expect(page.getByTestId('verdict-badge')).toHaveText('regressed');
    expect(lastOf(calls.reads).pathname).toBe('/api/perf/payments');
    expect(lastOf(calls.reads).search).toBe('?quantile=p95&env=demo&range=30d');
});

test('GIVEN only the keyboard WHEN tabbing to the target picker and the store button THEN both can be operated', async ({
    page,
}) => {
    const calls = await mockApi(page, { me: session('operator') });
    await page.goto('/performance');
    await expect(page.getByTestId('verdict-badge')).toHaveText('regressed');

    const tabTo = async (testId: string): Promise<void> => {
        for (let i = 0; i < 80; i += 1) {
            await page.keyboard.press('Tab');
            const focused = await page.evaluate(
                () => (document.activeElement as HTMLElement | null)?.dataset.testid ?? '',
            );
            if (focused === testId) return;
        }
        throw new Error(`${testId} was never reached with Tab`);
    };

    await tabTo('target-picker');
    await expect(page.getByLabel('Target')).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(page.getByLabel('Target')).toHaveValue('payments');
    await expect.poll(() => lastOf(calls.reads).pathname).toBe('/api/perf/payments');
    await expect(page).toHaveURL(/\/performance\?target=payments$/);
    await expect(page.getByLabel('Target')).toBeFocused();

    await tabTo('store-button');
    await expect(page.getByRole('button', { name: 'Store verdict' })).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId('store-status')).toHaveText('Verdict stored');
    expect(lastOf(calls.stores).pathname).toBe('/api/perf/payments/verdicts');
});

test('GIVEN the console navigation WHEN it renders THEN it links Performance to /performance right after Posture', async ({
    page,
}) => {
    await mockApi(page);
    await page.goto('/performance');

    const link = page.getByRole('navigation').getByRole('link', { name: 'Performance' });
    await expect(link).toHaveAttribute('href', '/performance');
    await expect(link).toHaveClass(/active/);
    const labels = await page.getByRole('navigation').getByRole('link').allTextContents();
    const posture = labels.findIndex((l) => l.includes('Posture'));
    expect(labels[posture + 1]).toContain('Performance');
});

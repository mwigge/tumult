import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { EVIL, expectShownAsText, installChartHooks, showTip } from './charts';

// The API is mocked. Charts render to canvas, so the heatmap, the gauge
// and the component bars are asserted through the option mirrored at
// window.__tumultCharts.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const header = {
    as_of_ns: 1_794_000_000_000_000_000,
    registry_version: '2026.11',
    lei: '529900T8BM49AURSDO55',
    entity_name: 'Demo Firm',
    profile: 'simplified',
    profile_label: 'simplified',
    profile_flags: [],
    legal_entities: [
        { lei: '529900T8BM49AURSDO55', name: 'Demo Firm', profile: 'simplified' },
        { lei: '506700GE1G29325QX363', name: 'Demo Bank', profile: 'full' },
    ],
    profile_overridden: false,
    registered_profile: 'simplified',
    scoped: false,
    scope_note: null,
    feeds: [
        { name: 'register', declared: true, connected: true, reason: null },
        {
            name: 'incidents',
            declared: true,
            connected: false,
            reason: 'declared in the org file, no incident record ingested',
        },
    ],
    params: { source: 'defaults', fallback: false, held: false, reason: null, overridden: [] },
    snapshot: { skipping: false, skipped: 0, reason: null },
    proof_days: { enabled: false, counted: 0, ignored: 0, note: null },
    register_state: 'ok',
    register_reason: null,
};

const cell = (
    article: string,
    entity_id: string,
    state: string,
    coverage: number | null,
    unknown: number,
    status_rank: number,
) => ({
    article,
    entity_id,
    coverage,
    unknown,
    state,
    status: ['none', 'designed', 'existing', 'operating'][status_rank],
    status_rank,
    gaps: status_rank < 2 ? 1 : 0,
});

const article = (key: string, title: string, coverage: number | null, unknown: number) => ({
    article: key,
    title,
    coverage,
    unknown,
    band: coverage === null ? 'unknown' : 'fair',
    na: 0,
    na_rows: 0,
    status: { meets: 1, gap: 1, unknown: 0, no_events: 0, na: 0 },
    requirements: [],
    sources: { otel_only: 0.5, tested: 0.25, manual: 0, none: 0.25, unknown: 0 },
});

const expectation = (
    id: string,
    art: string,
    expected: string,
    actual: string,
    gap: number,
    priority: number,
    days: number | null,
) => ({
    id,
    article: art,
    cite: `Art. ${art}(1)`,
    title: `Requirement ${id}`,
    weight: 2,
    expected,
    actual,
    gap,
    standing: gap > 0 ? 'gap' : 'meets',
    criticality: 1,
    priority,
    short: gap > 0 ? ['cif-0001'] : [],
    oldest_evidence_ns: 1_790_000_000_000_000_000,
    oldest_evidence_date: '2026-09-21',
    days_until_operating: days,
    evidence: { id: `ev-${id}`, source: 'run' },
    reason: null,
    scope_basis: art === '24' ? 'house' : 'regulation',
    window_days: art === '24' ? null : 365,
});

/** `GET /api/dora/24`: the rows of one article with the evidence behind them. */
const article24 = {
    ...header,
    disclaimer: 'Evidence, not a compliance verdict.',
    article: '24',
    title: 'General requirements for testing',
    entity: null,
    coverage: 0.55,
    unknown: 0.2,
    na_rows: 0,
    requirements: [
        {
            id: 'dora.art24.1',
            cite: 'Art. 24(1)',
            title: 'Requirement dora.art24.1',
            weight: 2,
            evidence: 'tested',
            window_days: null,
            scope: 'critical_or_important',
            scope_basis: 'house',
            strength: 'tested',
            coverage: 0.55,
            state: 'known',
            status: 'designed',
            expected: 'operating',
            gap: 2,
            reason: null,
            entities: [
                {
                    entity_id: 'cif-0001',
                    tier: 'critical',
                    state: 'known',
                    v: 0.55,
                    age_days: 12,
                    outcome: 'passed',
                    evidence_id: 'ev-dora.art24.1',
                    source: 'run',
                    status: 'designed',
                    streak_days: 3,
                    days_until_operating: null,
                    reason: null,
                    evidence: [
                        {
                            evidence_id: 'ev-dora.art24.1',
                            source: 'run',
                            kind: 'dr',
                            exercise_type: null,
                            experiment: 'payments-api restore',
                            at_ns: 1_793_000_000_000_000_000,
                            age_days: 12,
                            outcome: 'passed',
                            phi: 0.9,
                            o: 1,
                            s: 0.6,
                            v: 0.55,
                        },
                    ],
                },
                {
                    entity_id: 'cif-0002',
                    tier: 'important',
                    state: 'no_events',
                    v: null,
                    age_days: null,
                    outcome: null,
                    evidence_id: null,
                    source: null,
                    status: 'no_events',
                    streak_days: 0,
                    days_until_operating: null,
                    reason: 'no test in the window',
                    evidence: [],
                },
            ],
        },
    ],
};

const dora = (lei: string | null) => ({
    ...header,
    ...(lei === '506700GE1G29325QX363'
        ? { lei, entity_name: 'Demo Bank', profile: 'full', profile_label: 'full' }
        : {}),
    disclaimer: 'Evidence, not a compliance verdict.',
    by: 'entity',
    portfolio: { coverage: 0.62, unknown: 0.07, band: 'fair' },
    status: { meets: 3, gap: 2, unknown: 1, no_events: 1, na: 11 },
    rows: [
        { entity_id: 'cif-0001', label: 'Payments', tier: 'critical' },
        { entity_id: 'cif-0002', label: 'Reporting', tier: 'important' },
    ],
    functions: [
        {
            entity_id: 'cif-0001',
            name: 'Payments',
            tier: 'critical',
            registered: true,
            register_codes: ['F1'],
            org_unit: 'platform',
            rto_s: 3600,
            rpo_s: 300,
            last_assessed_ns: 1,
            last_assessed: '2026-10-01',
            coverage: 0.7,
            unknown: 0.1,
            band: 'fair',
        },
        {
            entity_id: 'cif-0002',
            name: 'Reporting',
            tier: 'important',
            registered: true,
            register_codes: ['F2'],
            org_unit: 'platform',
            rto_s: null,
            rpo_s: null,
            last_assessed_ns: null,
            last_assessed: null,
            coverage: null,
            unknown: 1,
            band: 'unknown',
        },
    ],
    articles: [
        article('16', 'Simplified ICT risk management framework', 0.7, 0.05),
        article('19', 'Reporting of major ICT-related incidents', null, 1),
        article('24', 'General requirements for testing', 0.55, 0.2),
    ],
    cells: [
        cell('16', 'cif-0001', 'known', 0.82, 0.25, 3),
        cell('16', 'cif-0002', 'known', 0.4, 0, 1),
        cell('19', 'cif-0001', 'unknown', null, 1, 0),
        cell('19', 'cif-0002', 'unknown', null, 1, 0),
        cell('24', 'cif-0001', 'known', 0.55, 0.2, 2),
        cell('24', 'cif-0002', 'no_events', null, 0, 0),
    ],
    expectations: [
        expectation('dora.art16.1.a', '16', 'existing', 'existing', 0, 0, 42),
        expectation('dora.art24.1', '24', 'operating', 'designed', 2, 4, null),
        expectation('dora.art16.1.c', '16', 'existing', 'designed', 1, 2, null),
    ],
    na: { count: 11, rows: [] },
    readiness: {
        functions: 2,
        registered: 2,
        with_rto: 1,
        with_rpo: 1,
        with_assessment: 1,
        complete: 1,
        register: {
            connected: true,
            reconciled_ns: 1_793_000_000_000_000_000,
            signed_by: 'cro@demo.example',
            signed_at: '2026-10-15',
            rows: 2,
        },
        unmapped_services: ['legacy-batch'],
    },
});

const node = (
    id: string,
    kind: string,
    parent: string | null,
    level: number,
    index: number,
    extra: Record<string, unknown> = {},
) => ({
    id,
    kind,
    label: id.split(':')[1] || 'organisation',
    parent,
    index,
    level,
    target: 3,
    own: null,
    pillars: {},
    rollup: { mean: index, weight: 1, children: 0 },
    unknown_share: 0,
    unknown: false,
    critical: false,
    cap: { rule: 'none', level: null, node_ids: [], origin: [], origin_state: null, reason: null },
    drag: null,
    note: null,
    ...extra,
});

const capped = {
    rule: 'weakest_critical_child',
    level: 1,
    node_ids: ['ent:cif-0001'],
    origin: ['svc:ledger-archive'],
    origin_state: 'unknown',
    reason: 'a critical child is at level 1',
};

const maturity = {
    ...header,
    scale: {
        internal: true,
        source: 'Internal house scale. DORA defines no maturity levels; this is not a regulatory rating.',
        levels: ['Initial', 'Repeatable', 'Defined', 'Measured', 'Improving'],
    },
    level: 2,
    index: 3.6,
    flat_level: 2,
    flat_index: 3.71,
    target: 3,
    headline: 'level 2 of target 3 (simplified), 25 % unknown',
    gate_level: 2,
    capped: true,
    cap: {
        rule: 'gate',
        level: 2,
        blocked_level: 3,
        component: 'evidence',
        reason: 'level 3 blocked by: restore-test evidence is 150 days old; 180 days are needed',
    },
    components: { conformance: 81, coverage: 64.5, slo: null, recovery: 70, loop: 55 },
    component_states: {
        conformance: { value: 81, state: 'value', reason: null },
        coverage: { value: 64.5, state: 'value', reason: null },
        slo: { value: null, state: 'unknown', reason: 'no SLO source connected' },
        recovery: { value: 70, state: 'value', reason: null },
        loop: { value: 55, state: 'value', reason: null },
    },
    weights: { conformance: 0.2, coverage: 0.3, slo: 0.25, recovery: 0.15, loop: 0.1 },
    conformance_shares: null,
    gates: [
        { level: 2, passed: true, failing: [] },
        { level: 3, passed: false, failing: ['restore-test evidence is 150 days old'] },
        { level: 4, passed: false, failing: ['SLO attainment (S) is unknown'] },
        { level: 5, passed: false, failing: ['learning loop (L) below 80'] },
    ],
    unknown_share: 0.25,
    flat_unknown_share: 0.4,
    history: [
        {
            quarter: '2026-Q1',
            state: 'no_data',
            as_of: null,
            as_of_ns: null,
            level: null,
            index: null,
            unknown_share: null,
            reason: null,
        },
        {
            quarter: '2026-Q2',
            state: 'stored',
            as_of: '2026-06-30',
            as_of_ns: 1,
            level: 1,
            index: 2.1,
            unknown_share: 0.5,
            reason: null,
        },
        {
            quarter: '2026-Q3',
            state: 'no_data',
            as_of: null,
            as_of_ns: null,
            level: null,
            index: null,
            unknown_share: null,
            reason: null,
        },
        {
            quarter: '2026-Q4',
            state: 'today',
            as_of: '2026-11-13',
            as_of_ns: 3,
            level: 2,
            index: 3.6,
            unknown_share: 0.25,
            reason: null,
        },
    ],
    rollup: node('org:', 'organisation', null, 1, 2.9, { cap: capped, unknown_share: 0.2 }),
    rollup_headline: 'level 1, capped by svc:ledger-archive',
    by: 'node',
    nodes: [
        node('org:', 'organisation', null, 1, 2.9, { cap: capped, unknown_share: 0.2 }),
        node('org:platform', 'org', 'org:', 1, 2.9, { cap: capped, unknown_share: 0.2 }),
        node('ent:cif-0001', 'entity', 'org:platform', 1, 2.6, {
            cap: { ...capped, node_ids: ['svc:ledger-archive'] },
            critical: true,
            unknown_share: 0.5,
        }),
        node('svc:payments-api', 'service', 'ent:cif-0001', 4, 4.2, { critical: true }),
        node('svc:ledger-archive', 'service', 'ent:cif-0001', 1, 1, {
            critical: true,
            unknown: true,
            unknown_share: 1,
            note: 'declared, no telemetry received',
        }),
        node('ent:cif-0002', 'entity', 'org:platform', 3, 3.4),
    ],
};

const tree = {
    name: 'company',
    path: '',
    kind: 'company',
    score: 62.5,
    band: 'fair',
    weight: 1,
    scored: 1,
    expected: 2,
    coverage: 0.5,
    weakest: null,
    children: [],
    delta: 0,
    sparkline: [],
};

/** Fields laid over the mocked documents of one test. */
interface Overrides {
    dora?: Record<string, unknown>;
    maturity?: Record<string, unknown>;
    /** Paths the API fails on, with the status it answers. */
    failing?: Record<string, number>;
}

async function mockApi(page: Page, requested: string[] = [], over: Overrides = {}) {
    await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const url = new URL(route.request().url());
        requested.push(url.pathname + url.search);
        const failed = over.failing?.[url.pathname];
        if (failed) {
            await route.fulfill({ status: failed, json: { error: 'the lake is not readable' } });
            return;
        }
        let body: unknown = {};
        if (url.pathname === '/api/me') body = me;
        else if (url.pathname === '/api/dora')
            body = { ...dora(url.searchParams.get('lei')), ...over.dora };
        else if (url.pathname === '/api/dora/24') body = article24;
        else if (url.pathname === '/api/maturity') body = { ...maturity, ...over.maturity };
        else if (url.pathname === '/api/scores/tree') body = tree;
        await route.fulfill({
            status: 200,
            contentType: 'application/json',
            body: JSON.stringify(body),
        });
    });
}

interface Mark {
    article: string;
    entity_id: string;
    text: string;
    hatch: number;
    fill: string | null;
}

async function heatmapMarks(page: Page): Promise<Mark[]> {
    return page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.['dora-heatmap'] as
            { series?: { data?: Mark[] }[] } | undefined;
        return (opt?.series?.[0]?.data ?? []).map((d) => ({
            article: d.article,
            entity_id: d.entity_id,
            text: d.text,
            hatch: d.hatch,
            fill: d.fill,
        }));
    });
}

const markOf = (marks: Mark[], art: string, entity: string) =>
    marks.find((m) => m.article === art && m.entity_id === entity);

test('the default view states level, cap, unknown share, parameters and register', async ({
    page,
}) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const head = page.getByTestId('dora-header');
    await expect(head.locator('.headline')).toHaveText(
        'level 2 of target 3 (simplified), 25 % unknown',
    );
    const panel = page.getByTestId('dora-maturity');
    await expect(panel.getByRole('heading', { level: 2 })).toContainText('25 % unknown');
    // The pillars alone rest on a different share, and both are named.
    await expect(panel).toContainText('pillars alone: unknown 40 %');
    // Nothing is announced when there is nothing to announce.
    await expect(page.getByTestId('score-params')).toContainText('house defaults');
    await expect(page.getByTestId('register-state')).toContainText('ok');
    for (const id of ['profile-override', 'scope-note', 'snapshot-skipping', 'proof-days']) {
        await expect(page.getByTestId(id)).toHaveCount(0);
    }
    await expect(head).toContainText('restore-test evidence is 150 days old');
    await expect(head.getByTestId('profile')).toHaveText('simplified');
    await expect(head).toContainText('Internal house scale');
    await expect(page.getByTestId('dora-rollup')).toContainText(
        'level 1, capped by svc:ledger-archive',
    );
});

test('no numeric tile renders without an unknown-share label', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    await expect(page.getByTestId('dora-header')).toBeVisible();
    const tiles = page.getByTestId('score-number');
    const count = await tiles.count();
    expect(count).toBeGreaterThanOrEqual(6);
    for (let i = 0; i < count; i++) {
        await expect(tiles.nth(i).getByTestId('unknown-share')).toHaveText(/^unknown \d+ %$/);
    }
    // Every number in a table of the view carries its share too.
    const figures = page.locator('[data-testid="dora-view"] [data-figure]');
    const n = await figures.count();
    expect(n).toBeGreaterThan(0);
    for (let i = 0; i < n; i++) {
        await expect(figures.nth(i)).toContainText(/unknown \d+ %/);
    }
    // A component without a value says so instead of showing zero.
    await expect(page.getByTestId('dora-maturity')).toContainText('no SLO source connected');
});

test('the heatmap hatches unknown, dashes no-events and toggles to evidence status', async ({
    page,
}) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    await expect.poll(async () => (await heatmapMarks(page)).length).toBe(6);
    let marks = await heatmapMarks(page);
    // Art. 19 has no incident feed: the whole column is hatched, no number.
    expect(markOf(marks, '19', 'cif-0001')).toMatchObject({ text: '?', hatch: 1, fill: null });
    expect(markOf(marks, '19', 'cif-0002')).toMatchObject({ text: '?', hatch: 1, fill: null });
    // No events in the window: a dash, not a zero.
    expect(markOf(marks, '24', 'cif-0002')).toMatchObject({ text: '–', hatch: 0, fill: null });
    // A scored cell: its coverage, with its unknown share hatched.
    expect(markOf(marks, '16', 'cif-0001')).toMatchObject({ text: '82', hatch: 0.25 });
    expect(markOf(marks, '16', 'cif-0002')?.fill).not.toBe(markOf(marks, '16', 'cif-0001')?.fill);
    await expect(page.getByTestId('dora-heatmap')).toContainText(
        '11 rows not applicable to profile simplified',
    );

    await page.getByRole('button', { name: 'Evidence status' }).click();
    await expect
        .poll(async () => markOf(await heatmapMarks(page), '16', 'cif-0001')?.text)
        .toBe('3');
    marks = await heatmapMarks(page);
    expect(markOf(marks, '16', 'cif-0002')?.text).toBe('1');
    expect(markOf(marks, '19', 'cif-0001')).toMatchObject({ text: '?', hatch: 1 });
});

test('the expectation table is sorted by gap, weight and criticality', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const rows = page.getByTestId('dora-expectations').locator('tbody tr');
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toContainText('Art. 24(1)');
    await expect(rows.nth(0)).toContainText('operating');
    await expect(rows.nth(0)).toContainText('designed');
    await expect(rows.nth(1)).toContainText('dora.art16.1.c');
    await expect(rows.nth(2)).toContainText('42');
    await expect(rows.nth(2)).toContainText('2026-09-21');
    // A row the house scoped, and not the regulation, says so.
    await expect(rows.nth(0).getByTestId('house-scope')).toHaveText('house scope');
    await expect(rows.nth(1).getByTestId('house-scope')).toHaveCount(0);
});

test('the evidence of a row opens in a drawer, not as raw JSON', async ({ page }) => {
    const requested: string[] = [];
    await mockApi(page, requested);
    await page.goto('/scores?view=dora');
    const table = page.getByTestId('dora-expectations');
    // Nothing in the view links to an API document.
    await expect(page.locator('[data-testid="dora-view"] a[href^="/api/"]')).toHaveCount(0);
    await table.getByRole('button', { name: 'ev-dora.art24.1' }).click();
    const drawer = page.getByRole('dialog', { name: 'Evidence behind Art. 24(1)' });
    await expect(drawer).toBeVisible();
    await expect(drawer).toContainText('Requirement dora.art24.1');
    await expect(drawer).toContainText('payments-api restore');
    await expect(drawer).toContainText('passed');
    await expect(drawer).toContainText('2026-10-26');
    // A function with no evidence says why instead of showing nothing.
    await expect(drawer).toContainText('no test in the window');
    expect(requested.some((r) => r.startsWith('/api/dora/24') && r.includes('lei=5299'))).toBe(
        true,
    );
    await drawer.getByRole('button', { name: 'Close' }).click();
    await expect(drawer).toHaveCount(0);
});

test('a label with markup is shown as text in the heatmap tooltip', async ({ page }) => {
    const base = dora(null);
    await mockApi(page, [], {
        dora: { rows: base.rows.map((r) => ({ ...r, label: EVIL })) },
    });
    await page.goto('/scores?view=dora');
    const tip = await showTip(page, 'dora-heatmap', { seriesIndex: 0, cell: [0, 0] });
    await expect(tip).toHaveClass(/dora-tooltip/);
    await expectShownAsText(page, tip, `Art. 16 · ${EVIL}`);
    await expectShownAsText(page, tip, 'coverage 82 %');
});

test('a failed read says so instead of showing numbers', async ({ page }) => {
    await mockApi(page, [], { failing: { '/api/maturity': 500 } });
    await page.goto('/scores?view=dora');
    await expect(page.getByText('Failed to load the DORA score')).toBeVisible();
    await expect(page.getByText('the lake is not readable')).toBeVisible();
    await expect(page.getByTestId('dora-view')).toHaveCount(0);
    await expect(page.getByTestId('score-number')).toHaveCount(0);
});

test('nothing on record is said in words, with no chart and no zero', async ({ page }) => {
    await mockApi(page, [], {
        dora: {
            lei: null,
            entity_name: null,
            legal_entities: [],
            rows: [],
            functions: [],
            cells: [],
            expectations: [],
        },
        maturity: { history: [], nodes: [], rollup: null, rollup_headline: null },
    });
    await page.goto('/scores?view=dora');
    const head = page.getByTestId('dora-header');
    await expect(head).toContainText('unregistered entity');
    await expect(head.getByLabel('Legal entity')).toBeDisabled();
    await expect(head.getByLabel('Legal entity')).toContainText('no legal entity registered');
    await expect(page.getByTestId('dora-heatmap')).toContainText('No functions on record');
    await expect(page.locator('[data-chart="dora-heatmap"]')).toHaveCount(0);
    await expect(page.getByTestId('dora-expectations')).toContainText(
        'No requirement applies to this profile',
    );
    await expect(page.locator('[data-chart="dora-history"]')).toHaveCount(0);
    await expect(page.getByTestId('dora-rollup')).toContainText('no nodes on record');
    await expect(page.getByTestId('dora-rollup')).toContainText('No nodes to roll up');
});

test('parameters held from the last valid org file are named with the reason', async ({ page }) => {
    await mockApi(page, [], {
        dora: {
            params: {
                source: 'last_valid_org_file',
                fallback: false,
                held: true,
                reason: 'score.operating_days must be within 1..=3650, got 99999',
                overridden: [{ key: 'operating_days', value: 30, default: 180 }],
            },
            register_state: 'invalid',
            register_reason: 'score.operating_days must be within 1..=3650, got 99999',
        },
    });
    await page.goto('/scores?view=dora');
    const params = page.getByTestId('score-params');
    await expect(params.locator('.badge.warn').first()).toHaveText('last valid file in force');
    await expect(params).toContainText('the org file on disk was refused');
    await expect(params).toContainText('score.operating_days must be within 1..=3650, got 99999');
    // What the held file overrides is still listed.
    await expect(params).toContainText('operating_days');
    await expect(params).toContainText('default 180');
    await expect(params).not.toContainText('defaults in force');
    await expect(page.getByTestId('snapshot-skipping')).toHaveCount(0);
});

test('a daily snapshot that is not stored is announced with its count', async ({ page }) => {
    const refused = 'score.band_good must be at most 1, got 7';
    const blocked = {
        params: {
            source: 'defaults',
            fallback: true,
            held: false,
            reason: refused,
            overridden: [],
        },
        snapshot: {
            skipping: true,
            skipped: 3,
            reason: `the daily snapshot is not stored while the default parameters stand in for a refused org file: ${refused}`,
        },
    };
    await mockApi(page, [], { dora: blocked, maturity: blocked });
    await page.goto('/scores?view=dora');
    const banner = page.getByTestId('snapshot-skipping');
    await expect(banner).toHaveAttribute('role', 'status');
    await expect(banner).toContainText('the daily snapshot is not stored while');
    await expect(banner).toContainText(refused);
    await expect(banner).toContainText('3 days skipped since the daemon started');
    await expect(page.getByTestId('score-params')).toContainText('defaults in force');
});

test('proof days in use are announced and marked in the history', async ({ page }) => {
    const note = 'proof days are in use: this is a test setting, not evidence of operation.';
    const proof = { proof_days: { enabled: true, counted: 2, ignored: 0, note } };
    const history = maturity.history.map((h) =>
        h.state === 'stored' ? { ...h, source: 'proof' } : h,
    );
    await mockApi(page, [], { dora: proof, maturity: { ...proof, history } });
    await page.goto('/scores?view=dora');
    const banner = page.getByTestId('proof-days');
    await expect(banner).toHaveAttribute('role', 'status');
    await expect(banner).toContainText(note);
    await expect(banner).toContainText('2 proof days counted');
    const rows = page.getByTestId('dora-history').locator('tbody tr');
    await expect(rows.nth(1)).toContainText('stored 2026-06-30 by the proof tool');
    await expect(rows.nth(3)).toHaveText(/today/);
    await expect(rows.nth(3)).not.toContainText('proof');
});

test('proof days in the lake that do not count are mentioned, not announced', async ({ page }) => {
    await mockApi(page, [], {
        dora: { proof_days: { enabled: false, counted: 0, ignored: 4, note: null } },
    });
    await page.goto('/scores?view=dora');
    await expect(page.getByTestId('proof-days')).toHaveCount(0);
    await expect(page.getByTestId('proof-days-ignored')).toContainText(
        '4 days stored by the proof tool are not counted',
    );
});

test('overridden parameters, a fallback and the register state are on the page', async ({
    page,
}) => {
    await mockApi(page, [], {
        dora: {
            params: {
                source: 'org_file',
                fallback: false,
                held: false,
                reason: null,
                overridden: [{ key: 'operating_days', value: 30, default: 180 }],
            },
            register_state: 'stale',
            register_reason: 'the org file changed and is not written to the lake yet',
        },
    });
    await page.goto('/scores?view=dora');
    const params = page.getByTestId('score-params');
    await expect(params).toContainText('operating_days');
    await expect(params).toContainText('30');
    await expect(params).toContainText('default 180');
    await expect(page.getByTestId('register-state')).toContainText('stale');
    await expect(page.getByTestId('register-state')).toContainText('not written to the lake yet');
    // A feed that is declared and has sent nothing is listed as such.
    const feeds = page.getByTestId('score-feeds');
    await expect(feeds).toContainText('incidents');
    await expect(feeds).toContainText('no incident record ingested');
});

test('a refused score section is named with the reason', async ({ page }) => {
    await mockApi(page, [], {
        dora: {
            params: {
                source: 'defaults',
                fallback: true,
                held: false,
                reason: 'score.band_good must be within 0 and 1, got 7',
                overridden: [],
            },
            register_state: 'invalid',
            register_reason: 'score.band_good must be within 0 and 1, got 7',
        },
    });
    await page.goto('/scores?view=dora');
    const params = page.getByTestId('score-params');
    await expect(params).toContainText('defaults in force');
    await expect(params).toContainText('score.band_good must be within 0 and 1, got 7');
    await expect(page.getByTestId('register-state')).toContainText('invalid');
});

test('a profile override and a partial view are announced above the numbers', async ({ page }) => {
    const partial = {
        profile: 'full',
        profile_label: 'full',
        profile_overridden: true,
        registered_profile: 'simplified',
        scoped: true,
        scope_note: 'Partial view: only the functions in your environments are listed.',
    };
    await mockApi(page, [], { dora: partial, maturity: partial });
    await page.goto('/scores?view=dora');
    const banner = page.getByTestId('profile-override');
    await expect(banner).toBeVisible();
    await expect(banner).toHaveAttribute('role', 'status');
    await expect(banner).toContainText('full');
    await expect(banner).toContainText('registered profile is simplified');
    await expect(page.getByTestId('scope-note')).toContainText('Partial view');
});

test('a quarter with no stored day is drawn as a gap and named', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const series = () =>
        page.evaluate(() => {
            const w = window as Window & { __tumultCharts?: Record<string, unknown> };
            const opt = w.__tumultCharts?.['dora-history'] as
                | { xAxis?: { data?: string[] }[]; series?: { data?: (number | null)[] }[] }
                | undefined;
            return {
                quarters: opt?.xAxis?.[0]?.data ?? [],
                levels: opt?.series?.[0]?.data ?? [],
                index: opt?.series?.[1]?.data ?? [],
            };
        });
    await expect.poll(series).toEqual({
        quarters: ['2026-Q1', '2026-Q2', '2026-Q3', '2026-Q4'],
        levels: [null, 1, null, 2],
        index: [null, 2.1, null, 3.6],
    });
    const rows = page.getByTestId('dora-history').locator('tbody tr');
    await expect(rows).toHaveCount(4);
    await expect(rows.nth(0)).toContainText('no data');
    await expect(rows.nth(1)).toContainText('stored 2026-06-30');
    await expect(rows.nth(1)).toContainText('unknown 50 %');
    await expect(rows.nth(3)).toContainText('today');
});

test('the toggles say which view is on and every chart has a name', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    await expect(page.getByTestId('dora-view')).toBeVisible();
    await expect(page.getByRole('button', { name: 'DORA and maturity' })).toHaveAttribute(
        'aria-pressed',
        'true',
    );
    await expect(page.getByRole('button', { name: 'Org scores' })).toHaveAttribute(
        'aria-pressed',
        'false',
    );
    const coverage = page.getByRole('button', { name: 'Coverage', exact: true });
    const status = page.getByRole('button', { name: 'Evidence status' });
    await expect(coverage).toHaveAttribute('aria-pressed', 'true');
    await expect(status).toHaveAttribute('aria-pressed', 'false');
    await status.click();
    await expect(coverage).toHaveAttribute('aria-pressed', 'false');
    await expect(status).toHaveAttribute('aria-pressed', 'true');
    for (const id of ['dora-heatmap', 'dora-gauge', 'dora-components', 'dora-history']) {
        const chart = page.locator(`[data-chart="${id}"]`);
        await expect(chart).toHaveAttribute('role', 'img');
        await expect(chart).toHaveAttribute('aria-label', /\S/);
    }
});

test('the heatmap has a complete table alternative', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const table = page.getByRole('table', { name: 'Articles by function, as a table' });
    await page.getByText('Show the heatmap as a table').click();
    await expect(table).toBeVisible();
    await expect(table.locator('thead th')).toHaveText([
        'Function',
        'Art. 16',
        'Art. 19',
        'Art. 24',
    ]);
    const rows = table.locator('tbody tr');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0).locator('th')).toHaveText('Payments');
    // Every cell of the heatmap, with the same words the tooltip uses.
    await expect(rows.nth(0).locator('td').nth(0)).toContainText('coverage 82 %');
    await expect(rows.nth(0).locator('td').nth(0)).toContainText('unknown 25 %');
    await expect(rows.nth(0).locator('td').nth(0)).toContainText('operating');
    await expect(rows.nth(0).locator('td').nth(1)).toContainText('no source connected');
    await expect(rows.nth(1).locator('td').nth(2)).toContainText('no events in window');
});

test('maturity shows weighted components, gates and a greyed index when capped', async ({
    page,
}) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const panel = page.getByTestId('dora-maturity');
    await expect(panel.locator('table.gates tbody tr')).toHaveCount(4);
    await expect(panel.locator('table.gates')).toContainText('SLO attainment (S) is unknown');
    await expect
        .poll(() =>
            page.evaluate(() => {
                const w = window as Window & { __tumultCharts?: Record<string, unknown> };
                const opt = w.__tumultCharts?.['dora-components'] as
                    | {
                          yAxis?: { data?: string[] }[];
                          series?: { data?: { value: number | null }[] }[];
                      }
                    | undefined;
                return {
                    names: opt?.yAxis?.[0]?.data ?? [],
                    values: (opt?.series?.[0]?.data ?? []).map((d) => d.value),
                };
            }),
        )
        .toEqual({
            names: [
                'Conformance (C) · weight 0.20',
                'Test coverage (T) · weight 0.30',
                'SLO attainment (S) · weight 0.25',
                'Recovery (R) · weight 0.15',
                'Learning loop (L) · weight 0.10',
            ],
            values: [81, 64.5, null, 70, 55],
        });
    // The index tile is greyed: a cap decides the level, not the index.
    const index = page.getByTestId('score-number').filter({ hasText: 'Maturity index' });
    await expect(index.locator('.value')).toHaveClass(/muted/);
    // The pillar-only index stays visible next to the rolled-up one.
    await expect(index).toContainText('pillars alone 3.71');
    const gauge = await page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.['dora-gauge'] as
            { series?: { data?: { value: number; capped: boolean }[] }[] } | undefined;
        return opt?.series?.[0]?.data?.[0];
    });
    expect(gauge).toMatchObject({ value: 3.6, capped: true });
});

test('the roll-up names the capping node down the chain', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const rows = page.getByTestId('dora-rollup').locator('tbody tr');
    await expect(rows).toHaveCount(6);
    await expect(rows.nth(0)).toContainText('level 1, capped by svc:ledger-archive');
    const silent = rows.filter({ hasText: 'ledger-archive' }).last();
    await expect(silent).toContainText('not instrumented');
    await expect(silent).toContainText('unknown 100 %');
});

test('readiness lists the functions with their register codes', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const panel = page.getByTestId('dora-readiness');
    const rows = panel.locator('table.functions tbody tr');
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText('cif-0001');
    await expect(rows.nth(0)).toContainText('F1');
    await expect(rows.nth(0)).toContainText('1 h');
    await expect(rows.nth(1)).toContainText('unknown 100 %');
    await expect(panel).toContainText('legacy-batch');
    await expect(panel).toContainText('cro@demo.example');
});

test('the legal entity selector reloads the score for that LEI', async ({ page }) => {
    const requested: string[] = [];
    await mockApi(page, requested);
    await page.goto('/scores?view=dora');
    await expect(page.getByTestId('dora-header')).toContainText('Demo Firm');
    await page.getByLabel('Legal entity').selectOption('506700GE1G29325QX363');
    await expect(page).toHaveURL(/lei=506700GE1G29325QX363/);
    await expect(page.getByTestId('dora-header').getByTestId('profile')).toHaveText('full');
    expect(requested.some((r) => r.startsWith('/api/dora') && r.includes('lei=506700'))).toBe(true);
    expect(requested.some((r) => r.startsWith('/api/maturity') && r.includes('lei=506700'))).toBe(
        true,
    );
});

test('charts follow the light theme', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const pointer = () =>
        page.evaluate(() => {
            const w = window as Window & { __tumultCharts?: Record<string, unknown> };
            const opt = w.__tumultCharts?.['dora-components'] as
                { series?: { data?: { itemStyle?: { color?: string } }[] }[] } | undefined;
            return opt?.series?.[0]?.data?.[0]?.itemStyle?.color;
        });
    await expect.poll(pointer).toBe('#5eb1ef');
    await page.getByRole('button', { name: /dark/ }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await expect.poll(pointer).toBe('#1f6fb2');
});

test('the navigation reaches the DORA view and the org scores stay', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores');
    await expect(page.getByText('Score — company')).toBeVisible();
    await expect(page.getByTestId('dora-view')).toHaveCount(0);
    await page.getByRole('link', { name: 'DORA' }).click();
    await expect(page).toHaveURL(/view=dora/);
    await expect(page.getByTestId('dora-view')).toBeVisible();
    await page.getByRole('button', { name: 'Org scores' }).click();
    await expect(page.getByText('Score — company')).toBeVisible();
});

import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

// The API is mocked; the map renders to canvas, so it is asserted through
// the option mirrored at window.__tumultCharts.starmap.

const DAY_NS = 86_400_000_000_000;
const NOW_NS = Date.now() * 1_000_000;

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const noRisks = {
    open: 0,
    suppressed: 0,
    worst: null,
    suppression_expires_ns: null,
    suppressed_by: null,
    suppression_reason: null,
    recommended: [],
};

function node(id: string, kind: string, label: string, parent: string | null, over = {}) {
    return {
        id,
        kind,
        label,
        parent,
        env: kind === 'org' || kind === 'entity' ? null : 'demo',
        tier: null,
        org_kind: kind === 'org' ? 'team' : null,
        criticality_weight: 1,
        conformance: null,
        last_seen_ns: null,
        posture: null,
        evidence: { runs: 0, failed: 0, manual: 0, last_run_ns: null, by_discipline: {} },
        gap: null,
        risks: noRisks,
        slo: null,
        slo_count: 0,
        dora: null,
        maturity: null,
        conflict: false,
        flags: [],
        rollup: null,
        declared: false,
        children: 0,
        members: null,
        ...over,
    };
}

const nodes = [
    node('org:', 'org', 'Company', null, { org_kind: 'company' }),
    node('org:platform', 'org', 'platform', 'org:', { org_kind: 'domain' }),
    node('org:platform/data-team', 'org', 'data-team', 'org:platform'),
    node('org:platform/runtime-team', 'org', 'runtime-team', 'org:platform'),
    node('org:(unassigned)', 'org', '(unassigned)', 'org:', { org_kind: 'unassigned' }),
    node('ent:cif-0003', 'entity', 'cif-0003', 'org:platform/data-team'),
    node('ent:cif-0001', 'entity', 'cif-0001', 'org:platform/runtime-team'),
    node('ent:cif-0005', 'entity', 'cif-0005', 'org:(unassigned)'),
    node('ent:CIF-9', 'entity', 'CIF-9', 'org:platform/data-team', {
        flags: ['malformed_id', 'owner_conflict'],
        rollup: {
            systems: 6,
            untested_systems: 5,
            stale_systems: 0,
            failed: 2,
            risks_open: 3,
            risks_suppressed: 1,
            gaps: 5,
        },
    }),
    node('sys:demo/message-queue', 'system', 'message-queue', 'ent:cif-0003', {
        evidence: {
            runs: 3,
            failed: 1,
            manual: 0,
            last_run_ns: NOW_NS - DAY_NS,
            by_discipline: { chaos: 'covered', dr: 'missing' },
        },
        risks: { ...noRisks, open: 1, worst: 'high', recommended: ['dr'] },
        posture: { score: 41.5, band: 'poor' },
    }),
    node('sys:demo/api-worker', 'system', 'api-worker', 'ent:cif-0001', {
        evidence: {
            runs: 2,
            failed: 0,
            manual: 0,
            last_run_ns: NOW_NS - DAY_NS,
            by_discipline: { chaos: 'covered' },
        },
        risks: {
            ...noRisks,
            suppressed: 1,
            suppression_expires_ns: NOW_NS + 60 * DAY_NS,
            suppressed_by: 'demo',
            suppression_reason: 'accepted for the demo',
        },
    }),
    node('sys:demo/session-cache', 'system', 'session-cache', 'ent:cif-0005', {
        gap: 'missing',
        declared: true,
        evidence: {
            runs: 0,
            failed: 0,
            manual: 0,
            last_run_ns: null,
            by_discipline: { chaos: 'missing', dr: 'missing' },
        },
    }),
    node(
        'wl:demo/statefulset/message-queue',
        'workload',
        'message-queue',
        'sys:demo/message-queue',
    ),
    node('def:name/queue-dependency-loss', 'def', 'queue-dependency-loss', null, { env: null }),
];

const edge = (from: string, rel: string, to: string, source = 'declared') => ({
    id: `${from}|${rel}|${to}`,
    from,
    to,
    rel,
    source,
});

const edges = [
    edge('org:', 'owns', 'org:platform'),
    edge('org:platform', 'owns', 'org:platform/data-team'),
    edge('org:platform', 'owns', 'org:platform/runtime-team'),
    edge('org:platform/data-team', 'owns', 'ent:cif-0003'),
    edge('org:platform/runtime-team', 'owns', 'ent:cif-0001'),
    edge('sys:demo/message-queue', 'implements', 'ent:cif-0003'),
    edge('sys:demo/api-worker', 'implements', 'ent:cif-0001'),
    edge('sys:demo/session-cache', 'implements', 'ent:cif-0005'),
    edge('sys:demo/message-queue', 'runs_on', 'wl:demo/statefulset/message-queue'),
    edge('sys:demo/message-queue', 'tested_by', 'def:name/queue-dependency-loss', 'evidence'),
];

const UNKNOWN = 'sys:demo/nope';

function starmap(query: URLSearchParams) {
    const found = query.get('focus') !== UNKNOWN;
    return { ...whole(query), nodes: found ? nodes : [], edges: found ? edges : [] };
}

const whole = (query: URLSearchParams) => ({
    as_of_ns: NOW_NS,
    range: query.get('range') ?? '7d',
    env: query.get('env'),
    focus: query.get('focus'),
    focus_found: query.get('focus') ? query.get('focus') !== UNKNOWN : null,
    depth: query.get('focus') ? Number(query.get('depth') ?? 2) : null,
    disclaimer: 'Evidence toward a requirement, not a statement of compliance.',
    truncated: false,
    sources: {
        slo: 'unavailable',
        dora: 'unavailable',
        maturity: 'unavailable',
        conformance: 'ok',
        graph: 'ok',
    },
    maturity: null,
    counts: {
        nodes: nodes.length,
        edges: edges.length,
        pruned: 0,
        clustered: 0,
        risks_open: 1,
        risks_suppressed: 1,
        risks_unplaced: 2,
        gaps: 1,
    },
    clusters: [],
});

const run = (id: string, name: string, status: string) => ({
    id,
    name,
    started_ns: NOW_NS - DAY_NS,
    duration_ns: 2_000_000_000,
    trace_id: null,
    target_system: 'message-queue',
    target_technology: 'queue',
    target_environment: 'demo',
    status,
    deviations: null,
    duration_ms: '2000',
    faults: null,
    origin: 'automated',
    review_status: null,
});

interface Item {
    name: string;
    category: string;
    itemStyle: { color?: string; borderType?: unknown; opacity?: number; borderColor?: string };
}

interface Mirror {
    series: { data: Item[]; links: unknown[]; label: { color: string }; zoom?: number }[];
}

/** What a test run installs so charts mirror their option; absent in production. */
async function installChartHook(page: Page) {
    await page.addInitScript(() => {
        (window as Window & { __tumultCharts?: Record<string, unknown> }).__tumultCharts = {};
    });
}

async function mock(page: Page, seen: URL[], hook = true, over: Record<string, unknown> = {}) {
    if (hook) await installChartHook(page);
    await page.route('**/api/**', async (route) => {
        const url = new URL(route.request().url());
        seen.push(url);
        const path = url.pathname;
        let body: unknown = {};
        if (path === '/api/me') body = me;
        else if (path === '/api/starmap') body = { ...starmap(url.searchParams), ...over };
        else if (path === '/api/experiments')
            body = {
                count: 3,
                experiments: [
                    run('run-1', 'queue-dependency-loss', 'deviated'),
                    run('run-2', 'message-queue-kill', 'completed'),
                    run('run-3', 'queue-dependency-loss-twice', 'completed'),
                ],
            };
        else if (path === '/api/manual/experiments') body = { records: [] };
        else if (path === '/api/risks')
            body = {
                as_of_ns: NOW_NS,
                risks: [
                    {
                        id: 'r_1',
                        rule: 'latest_run_failed',
                        severity: 'high',
                        rank: 80,
                        title: 'Latest chaos run failed',
                        detail: 'queue-dependency-loss deviated',
                        target: 'message-queue',
                        environment: 'demo',
                        subject: 'message-queue',
                        recommended_kind: null,
                        evidence: [],
                        suppression: null,
                    },
                ],
                suppressed: [],
                summary: { total: 1, critical: 0, high: 1, medium: 0, low: 0, suppressed: 0 },
                coverage: { kinds: [], targets: [], portfolio: [] },
            };
        else if (path === '/api/posture')
            body = {
                as_of_ns: NOW_NS,
                posture: { score: 41.5, band: 'poor', targets: [] },
                summary: { total: 1, critical: 0, high: 1, medium: 0, low: 0, suppressed: 0 },
                coverage: { kinds: [], targets: [], portfolio: [] },
                top_risks: [],
                nfrs: [],
            };
        else if (path.startsWith('/api/gate/'))
            body = {
                target: 'message-queue',
                score: 41.5,
                band: 'poor',
                verdict: 'fail',
                reasons: ['latest run deviated'],
                thresholds: { min_score: 70, max_age_days: 30 },
                as_of_ns: NOW_NS,
                evidence: {
                    runs: 3,
                    last_run_id: 'run-1',
                    last_run_at_ns: NOW_NS - DAY_NS,
                    last_outcome: 'deviated',
                    experiments: [],
                },
            };
        await route.fulfill({
            status: 200,
            contentType: 'application/json',
            body: JSON.stringify(body),
        });
    });
}

const mirror = (page: Page) =>
    page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const option = w.__tumultCharts?.starmap as Mirror | undefined;
        const series = option?.series?.[0];
        if (!series?.data?.length) return null;
        return {
            nodes: series.data.map((d) => ({
                name: d.name,
                category: d.category,
                itemStyle: d.itemStyle,
            })),
            links: series.links.length,
            labelColor: series.label.color,
            zoom: series.zoom ?? 1,
        };
    });

async function drawn(page: Page) {
    await expect.poll(async () => (await mirror(page))?.nodes.length ?? 0).toBeGreaterThan(0);
    const state = await mirror(page);
    if (!state) throw new Error('the star map chart is not mirrored');
    return state;
}

test('the star map draws every node with its evidence state', async ({ page }) => {
    const seen: URL[] = [];
    await mock(page, seen);
    await page.goto('/starmap');
    await expect(page.getByRole('heading', { name: 'Star Map' })).toBeVisible();

    const state = await drawn(page);
    expect(state.nodes.length).toBe(nodes.length);
    expect(state.links).toBe(edges.length);

    const queue = state.nodes.find((n) => n.name === 'sys:demo/message-queue');
    expect(queue?.category).toBe('system');

    const gap = state.nodes.find((n) => n.name === 'sys:demo/session-cache');
    expect(gap?.itemStyle.borderType).toBe('dashed');
    expect(gap?.itemStyle.color).toBe('transparent');

    const suppressed = state.nodes.find((n) => n.name === 'sys:demo/api-worker');
    expect(suppressed?.itemStyle.opacity).toBeLessThan(0.5);

    // Suppressed is never the same as absent: the legend counts both.
    await expect(page.getByTestId('starmap-legend')).toContainText('1 open');
    await expect(page.getByTestId('starmap-legend')).toContainText('1 suppressed');
    await expect(page.getByTestId('starmap-legend')).toContainText('1 gap');
    await expect(page.getByTestId('starmap-legend')).toContainText('2 risks not on the map');
    await expect(page.getByTestId('starmap-legend')).not.toContainText('incomplete');

    const request = seen.find((u) => u.pathname === '/api/starmap');
    expect(request?.searchParams.get('range')).toBe('7d');
});

test('the star map follows the theme', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap');
    expect((await drawn(page)).labelColor).toBe('#8b98a5');

    const toggle = page.locator('button.theme');
    for (let i = 0; i < 3; i += 1) {
        if ((await page.locator('html').getAttribute('data-theme')) === 'light') break;
        await toggle.click();
    }
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await expect.poll(async () => (await mirror(page))?.labelColor).toBe('#5b6772');

    const gap = (await drawn(page)).nodes.find((n) => n.name === 'sys:demo/session-cache');
    expect(gap?.itemStyle.borderColor).toBe('#5b6772');
});

test('lens, selection and focus round-trip through the URL', async ({ page }) => {
    const seen: URL[] = [];
    await mock(page, seen);
    await page.goto('/starmap?lens=risk&sel=sys%3Ademo%2Fmessage-queue&focus=ent%3Acif-0003');

    await drawn(page);
    await expect(page.getByRole('button', { name: 'risk', exact: true })).toHaveClass(/active/);
    const request = seen.find((u) => u.pathname === '/api/starmap');
    expect(request?.searchParams.get('focus')).toBe('ent:cif-0003');

    // The selection opens the evidence drawer, which loads from existing endpoints.
    const drawer = page.getByTestId('starmap-drawer');
    await expect(drawer).toContainText('message-queue');
    await expect(drawer).toContainText('queue-dependency-loss');
    await expect(
        drawer.getByRole('link', { name: 'queue-dependency-loss', exact: true }),
    ).toHaveAttribute('href', '/experiments/run-1');
    await expect(drawer).toContainText('Latest chaos run failed');
    const runs = seen.find((u) => u.pathname === '/api/experiments');
    expect(runs?.searchParams.get('target')).toBe('message-queue');

    // The selected node carries the accent border in the risk lens.
    const selected = (await drawn(page)).nodes.find((n) => n.name === 'sys:demo/message-queue');
    expect(selected?.itemStyle.borderColor).toBe('#5eb1ef');
    expect(selected?.itemStyle.color).toBe('#ef4444');

    // Switching lens keeps the selection and the focus.
    await page.getByRole('button', { name: 'conformance', exact: true }).click();
    await expect(page).toHaveURL(/lens=conformance/);
    await expect(page).toHaveURL(/sel=sys%3Ademo%2Fmessage-queue/);
    await expect(page).toHaveURL(/focus=ent%3Acif-0003/);

    // The drawer can move the focus to the selected node ...
    await drawer.getByRole('button', { name: 'Focus here' }).click();
    await expect(page).toHaveURL(/focus=sys%3Ademo%2Fmessage-queue/);
    await expect
        .poll(() =>
            seen.some(
                (u) =>
                    u.pathname === '/api/starmap' &&
                    u.searchParams.get('focus') === 'sys:demo/message-queue',
            ),
        )
        .toBe(true);

    // ... closing it clears the selection, and the focus chip clears the focus.
    await drawer.getByRole('button', { name: 'Close' }).click();
    await expect(page).not.toHaveURL(/sel=/);
    await expect(page.getByTestId('starmap-drawer')).toHaveCount(0);
    await page.getByRole('button', { name: 'Clear focus' }).click();
    await expect(page).not.toHaveURL(/focus=/);

    // The range switch offers thirty days.
    await page.getByRole('button', { name: '30d', exact: true }).click();
    await expect(page).toHaveURL(/range=30d/);
});

test('/topology redirects to the star map and keeps the range', async ({ page }) => {
    await mock(page, []);
    await page.goto('/topology?range=14d');
    await expect(page).toHaveURL(/\/starmap\?range=14d$/);
    await expect(page.getByRole('heading', { name: 'Star Map' })).toBeVisible();
    await expect(page.getByRole('link', { name: /Star Map/ })).toBeVisible();
});

test('the sidebar keeps the theme button in view on a short window', async ({ page }) => {
    await mock(page, []);
    await page.setViewportSize({ width: 1280, height: 420 });
    await page.goto('/starmap');
    const toggle = page.locator('button.theme');
    await expect(toggle).toBeInViewport({ ratio: 1 });
    // The navigation scrolls on its own, so its last entry can still be reached.
    const last = page.locator('nav.nav a').last();
    await last.scrollIntoViewIfNeeded();
    await expect(last).toBeInViewport({ ratio: 1 });
    await expect(toggle).toBeInViewport({ ratio: 1 });
});

test('the map can be read and driven from the keyboard', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap');
    await drawn(page);

    // The canvas is described for assistive technology ...
    await expect(page.locator('[data-chart="starmap"]')).toHaveAttribute(
        'aria-label',
        /Star map of 14 nodes and 10 edges/,
    );

    // ... and every node is a button in a list next to it.
    const list = page.getByRole('region', { name: 'nodes' });
    await expect(list.getByRole('button')).toHaveCount(nodes.length);
    const filter = list.getByRole('searchbox', { name: 'Filter nodes' });
    await filter.focus();
    await page.keyboard.type('message-queue');
    await expect(list.getByRole('button')).toHaveCount(2);

    await page.keyboard.press('Tab');
    const first = list.getByRole('button').first();
    await expect(first).toBeFocused();
    await expect(first).toContainText('message-queue');
    await page.keyboard.press('Enter');
    await expect(page).toHaveURL(/sel=sys%3Ademo%2Fmessage-queue/);
    await expect(first).toHaveAttribute('aria-pressed', 'true');

    // Opening the drawer moves the focus into it; Escape closes it and
    // hands the focus back to the button that opened it.
    const drawer = page.getByTestId('starmap-drawer');
    await expect(drawer.getByRole('heading', { name: 'message-queue' })).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(drawer).toHaveCount(0);
    await expect(page).not.toHaveURL(/sel=/);
    await expect(first).toBeFocused();
});

test('a map built from a capped read says it is incomplete', async ({ page }) => {
    await mock(page, [], true, { truncated: true });
    await page.goto('/starmap');
    await drawn(page);
    await expect(page.getByTestId('starmap-legend')).toContainText(
        'incomplete: a source table is longer than its row limit',
    );
});

test('the drawer shows only the runs of the selected definition', async ({ page }) => {
    const seen: URL[] = [];
    await mock(page, seen);
    await page.goto('/starmap?range=30d&sel=def%3Aname%2Fqueue-dependency-loss');
    const drawer = page.getByTestId('starmap-drawer');
    await expect(drawer.getByRole('link', { name: 'queue-dependency-loss' })).toHaveCount(1);
    await expect(drawer).not.toContainText('queue-dependency-loss-twice');
    await expect(drawer).not.toContainText('message-queue-kill');
    // The run list covers the same thirty days as the map.
    await expect(drawer).toContainText('Runs, last 30d');
    const runs = seen.find((u) => u.pathname === '/api/experiments');
    expect(runs?.searchParams.get('range')).toBe('30d');
    expect(runs?.searchParams.get('q')).toBe('queue-dependency-loss');
});

test('the drawer explains flags and what an entity hides', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap?sel=ent%3ACIF-9');
    const drawer = page.getByTestId('starmap-drawer');
    await expect(drawer).toContainText('the entity id does not follow the agreed form');
    await expect(drawer).toContainText('telemetry and org.yaml name different owners');
    await expect(drawer).toContainText('5 of 6 systems untested');
    await expect(drawer).toContainText('5 gaps, 2 failed runs, 3 open risks, 1 suppressed');
});

test('a focus that is not on the map says so', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap?focus=sys%3Ademo%2Fnope');
    await expect(page.getByText('sys:demo/nope is unknown or out of scope.')).toBeVisible();
    await page.getByRole('button', { name: 'Clear focus' }).click();
    await drawn(page);
});

test('changing the focus resets zoom and centre', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap?sel=sys%3Ademo%2Fmessage-queue');
    await drawn(page);
    const box = await page.locator('[data-chart="starmap"]').boundingBox();
    if (!box) throw new Error('the chart has no box');
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.wheel(0, -600);
    await expect.poll(async () => (await mirror(page))?.zoom ?? 1).toBeGreaterThan(1);

    // A lens change keeps what the user did to the view ...
    await page.getByRole('button', { name: 'risk', exact: true }).click();
    await expect(page).toHaveURL(/lens=risk/);
    expect((await drawn(page)).zoom).toBeGreaterThan(1);

    // ... a new focus is a new map and starts from the whole of it.
    await page.getByTestId('starmap-drawer').getByRole('button', { name: 'Focus here' }).click();
    await expect(page).toHaveURL(/focus=sys%3Ademo%2Fmessage-queue/);
    await expect.poll(async () => (await mirror(page))?.zoom).toBe(1);
});

test('charts are not mirrored unless a test asked for it', async ({ page }) => {
    await mock(page, [], false);
    await page.goto('/starmap');
    await expect(page.getByTestId('starmap-legend')).toBeVisible();
    const hook = await page.evaluate(
        () => typeof (window as Window & { __tumultCharts?: unknown }).__tumultCharts,
    );
    expect(hook).toBe('undefined');
});

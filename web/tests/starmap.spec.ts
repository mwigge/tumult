import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { COMPLIANCE_SCORE, COMPLIANCE_SCORE_LINE } from '../src/lib/score-names';
import { EVIL, expectShownAsText, installChartHooks, showTip } from './charts';
import { demoStar } from './starmap-demo';

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
            by_discipline: { chaos: 'covered', robustness: 'missing' },
        },
        risks: { ...noRisks, open: 1, worst: 'high', recommended: ['robustness'] },
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
const map = {
    disclaimer: 'Evidence toward a requirement, not a statement of compliance.',
};

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
    disclaimer: map.disclaimer,
    truncated: false,
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
    duration_ns: 5_400_000_000,
    trace_id: null,
    target_system: 'message-queue',
    target_technology: 'queue',
    target_environment: 'demo',
    status,
    deviations: null,
    duration_ms: '2000',
    journal_only: false,
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

const runs = [
    { ...run('run-1', 'queue-dependency-loss', 'deviated'), journal_only: true },
    run('run-2', 'message-queue-kill', 'completed'),
    run('run-3', 'queue-dependency-loss-twice', 'completed'),
];

/** The run list as the daemon filters it: `name` is exact, `q` a substring. */
function experiments(query: URLSearchParams) {
    const name = query.get('name');
    const q = query.get('q') ?? '';
    const rows = runs.filter((r) => (name === null ? r.name.includes(q) : r.name === name));
    return { count: rows.length, experiments: rows };
}

async function mock(page: Page, seen: URL[], hook = true, over: Record<string, unknown> = {}) {
    if (hook) await installChartHooks(page);
    await page.route('**/api/**', async (route) => {
        const url = new URL(route.request().url());
        seen.push(url);
        const path = url.pathname;
        let body: unknown = {};
        if (path === '/api/me') body = me;
        else if (path === '/api/starmap') body = { ...starmap(url.searchParams), ...over };
        else if (path === '/api/experiments') body = experiments(url.searchParams);
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
    // Every API edge, plus a plain line for each parent no edge names:
    // (unassigned) under the company, and the two entities without an owner edge.
    expect(state.links).toBe(edges.length + 3);

    const queue = state.nodes.find((n) => n.name === 'sys:demo/message-queue');
    expect(queue?.category).toBe('system');

    const gap = state.nodes.find((n) => n.name === 'sys:demo/session-cache');
    expect(gap?.itemStyle.borderType).toBe('dashed');
    expect(gap?.itemStyle.color).toBe('#151b21');

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

const SYSTEM_FILL = '#7B6CD9';

interface Drawn {
    id: string;
    x: number;
    y: number;
    symbol: string;
    category: string;
    itemStyle: { opacity?: number };
}

interface DrawnLink {
    id: string;
    source: string;
    target: string;
    symbol: string[];
    label: { show: boolean; formatter?: string };
    emphasis: { label: { show: boolean } };
    lineStyle: { color: string; type: string | number[]; opacity: number; curveness: number };
}

/** The whole graph as the chart holds it: what a reader sees on the canvas. */
const graph = (page: Page) =>
    page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const option = w.__tumultCharts?.starmap as
            { series: { data: Drawn[]; links: DrawnLink[] }[] } | undefined;
        const series = option?.series?.[0];
        return { data: series?.data ?? [], links: series?.links ?? [] };
    });

async function mockDemo(page: Page) {
    const demo = demoStar(NOW_NS);
    await mock(page, [], true, demo);
    return demo;
}

test('the demo-sized map reads like a graph: round nodes apart, named arrows', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    const demo = await mockDemo(page);
    await page.goto('/starmap');
    await drawn(page);
    const { data, links } = await graph(page);
    expect(data.length).toBe(51);
    expect(demo.edges.length).toBe(38);

    // Every node is a circle, and no two sit closer than a caption needs:
    // 112 side by side, 84 above each other.
    expect(new Set(data.map((d) => d.symbol))).toEqual(new Set(['circle']));
    let closest = Infinity;
    for (let i = 0; i < data.length; i += 1) {
        for (let j = i + 1; j < data.length; j += 1) {
            const [dx, dy] = [data[i].x - data[j].x, data[i].y - data[j].y];
            closest = Math.min(closest, Math.hypot(dx / 112, dy / 84));
        }
    }
    expect(closest).toBeGreaterThanOrEqual(0.999);

    // Every API edge is an arrow that carries the relation the API named.
    // It is written where the edge has room for it, at least on half of
    // them, and on hover everywhere.
    const named = links.filter((l) => !l.id.startsWith('spine|'));
    expect(named.length).toBe(38);
    for (const link of named) {
        expect(link.symbol, link.id).toEqual(['none', 'arrow']);
        expect(link.label.formatter, link.id).toBe(link.id.split('|')[1].toUpperCase());
        expect(link.emphasis.label.show, link.id).toBe(true);
    }
    expect(named.filter((l) => l.label.show).length).toBeGreaterThanOrEqual(19);
    // Each relation has its own line: colour and dash together.
    const lines = new Map(named.map((l) => [l.id.split('|')[1], JSON.stringify(l.lineStyle.type)]));
    const colours = new Map(named.map((l) => [l.id.split('|')[1], l.lineStyle.color]));
    expect(new Set(colours.values()).size).toBe(colours.size);
    expect(new Set(lines.values()).size).toBeGreaterThan(1);
    // A test and what it broke join the same two nodes: they bend apart.
    const broke = named.find((l) => l.label.formatter === 'BROKE');
    expect(broke?.lineStyle.curveness).not.toBe(0);

    // No node floats free: a parent the API joins by no edge gets a PART_OF
    // arrow from its child, and the one definition that never ran is the only
    // node without a link.
    const spine = links.filter((l) => l.id.startsWith('spine|'));
    expect(spine.length).toBeGreaterThan(0);
    expect(spine.every((l) => l.symbol[1] === 'arrow' && l.label.formatter === 'PART_OF')).toBe(
        true,
    );
    const broker = spine.find((l) => l.id === 'spine|cmp:demo/message-queue/broker');
    expect([broker?.source, broker?.target]).toEqual([
        'cmp:demo/message-queue/broker',
        'sys:demo/message-queue',
    ]);
    const linked = new Set(links.flatMap((l) => [l.source, l.target]));
    expect(data.filter((d) => !linked.has(d.id)).map((d) => d.id)).toEqual([
        'def:name/never-ran dns outage',
    ]);
});

test('overlays reserved for later are not listed under the map', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap');
    await drawn(page);
    await expect(page.getByText(map.disclaimer)).toBeVisible();
    await expect(page.getByText(/not available yet/i)).toHaveCount(0);
    await expect(page.getByText(/could not load/i)).toHaveCount(0);
});

test('the legend is chips per kind and a key, and a chip hides its kind', async ({ page }) => {
    await mockDemo(page);
    await page.goto('/starmap');
    await drawn(page);

    const kinds = page.getByTestId('starmap-kinds');
    await expect(kinds.getByRole('button')).toHaveCount(6);
    await expect(kinds.getByRole('button', { name: /org unit\s*9/ })).toBeVisible();
    await expect(kinds.getByRole('button', { name: /system\s*11/ })).toBeVisible();

    const chip = kinds.getByRole('button', { name: /experiment\s*12/ });
    await expect(chip).toHaveAttribute('aria-pressed', 'true');
    await chip.click();
    await expect(chip).toHaveAttribute('aria-pressed', 'false');
    await expect.poll(async () => (await graph(page)).data.length).toBe(39);
    expect((await graph(page)).links.some((l) => l.label.formatter === 'TESTED_BY')).toBe(false);
    await chip.click();
    await expect.poll(async () => (await graph(page)).data.length).toBe(51);

    // The key says what a ring means; shape no longer carries anything.
    const key = page.getByTestId('starmap-key');
    for (const text of ['runs passed', 'a run failed', 'no evidence', 'stale evidence']) {
        await expect(key).toContainText(text);
    }
    await expect(key).toContainText('only suppressed risks');
    await expect(key).not.toContainText('sits under');
    await page.getByRole('button', { name: 'risk', exact: true }).click();
    await expect(key).toContainText('high or critical risk');
    await expect(key).not.toContainText('runs passed');
});

test('a chip per relation shows its line and count, and hides the relation', async ({ page }) => {
    const demo = await mockDemo(page);
    await page.goto('/starmap');
    await drawn(page);

    const rels = page.getByTestId('starmap-rels');
    const present = [...new Set(demo.edges.map((e) => e.rel))];
    await expect(rels.getByRole('button')).toHaveCount(present.length + 1);
    for (const rel of present) {
        const count = demo.edges.filter((e) => e.rel === rel).length;
        const name = new RegExp(`^${rel.toUpperCase()}\\s*${count}$`);
        await expect(rels.getByRole('button', { name })).toBeVisible();
    }
    const spines = (await graph(page)).links.filter((l) => l.id.startsWith('spine|')).length;
    await expect(
        rels.getByRole('button', { name: new RegExp(`^PART_OF\\s*${spines}$`) }),
    ).toBeVisible();
    // The sample line is drawn in the colour of the relation on the map.
    const tested = (await graph(page)).links.find((l) => l.id.split('|')[1] === 'tested_by');
    const chip = rels.getByRole('button', { name: /^TESTED_BY/ });
    await expect(chip.locator('line')).toHaveAttribute('stroke', tested?.lineStyle.color ?? '');

    await chip.click();
    await expect(chip).toHaveAttribute('aria-pressed', 'false');
    await expect
        .poll(async () => (await graph(page)).links.some((l) => l.id.split('|')[1] === 'tested_by'))
        .toBe(false);
    expect((await graph(page)).data.length).toBe(51);
    await chip.click();
    await expect
        .poll(async () => (await graph(page)).links.some((l) => l.id.split('|')[1] === 'tested_by'))
        .toBe(true);
});

/** Ids of the links the chart lit with the node under the pointer. */
const litLinks = (page: Page) =>
    page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const lit = (w.__tumultCharts?.['starmap#lit'] ?? []) as number[];
        const option = w.__tumultCharts?.starmap as { series: { links: DrawnLink[] }[] };
        return lit.map((i) => option.series[0].links[i].id);
    });

/** Moves the pointer onto node `id`, searching around where it should be drawn. */
async function hoverNode(page: Page, id: string) {
    const at = await page.evaluate((id) => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const option = w.__tumultCharts?.starmap as { series: { data: Drawn[] }[] };
        const data = option.series[0].data;
        const r = (
            document.querySelector('[data-chart=starmap]') as HTMLElement
        ).getBoundingClientRect();
        const xs = data.map((d) => d.x);
        const ys = data.map((d) => d.y);
        const [x0, x1, y0, y1] = [
            Math.min(...xs),
            Math.max(...xs),
            Math.min(...ys),
            Math.max(...ys),
        ];
        const scale = Math.min((r.width - 140) / (x1 - x0), (r.height - 96) / (y1 - y0));
        const node = data.find((d) => d.id === id) ?? data[0];
        return [
            r.x + r.width / 2 + (node.x - (x0 + x1) / 2) * scale,
            r.y + r.height / 2 + (node.y - (y0 + y1) / 2) * scale,
        ];
    }, id);
    for (let ring = 0; ring < 6; ring += 1) {
        for (let k = 0; k < Math.max(1, ring * 6); k += 1) {
            const a = (k / Math.max(1, ring * 6)) * 2 * Math.PI;
            await page.mouse.move(at[0] + Math.cos(a) * ring * 6, at[1] + Math.sin(a) * ring * 6);
            const lit = await litLinks(page);
            if (lit.length > 0) return lit;
        }
    }
    return [];
}

test('hovering a node writes the relation on every one of its links', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await mockDemo(page);
    await page.goto('/starmap');
    await drawn(page);
    const id = 'sys:demo/message-queue';
    const { links } = await graph(page);
    const own = links.filter((l) => l.source === id || l.target === id).map((l) => l.id);
    // At rest some of them are left without a name.
    expect(links.filter((l) => own.includes(l.id) && !l.label.show).length).toBeGreaterThan(0);

    expect(await hoverNode(page, id)).toEqual(own);
    // Those links are in their hover state, and on hover every link is named.
    for (const link of links.filter((l) => own.includes(l.id))) {
        expect(link.emphasis.label.show, link.id).toBe(true);
    }
    await page.mouse.move(5, 5);
    await expect.poll(() => litLinks(page)).toEqual([]);
});

test('selecting a node writes the relation on every one of its links', async ({ page }) => {
    await mockDemo(page);
    await page.goto('/starmap?sel=sys%3Ademo%2Fmessage-queue');
    await drawn(page);
    const id = 'sys:demo/message-queue';
    const { links } = await graph(page);
    const own = links.filter((l) => l.source === id || l.target === id);
    expect(own.length).toBeGreaterThan(3);
    for (const link of own) expect(link.label.show, link.id).toBe(true);
});

test('selecting a node keeps its neighbourhood and dims the rest', async ({ page }) => {
    await mockDemo(page);
    await page.goto('/starmap?sel=sys%3Ademo%2Fmessage-queue');
    await drawn(page);
    const { data, links } = await graph(page);
    const opacity = (id: string) => data.find((d) => d.id === id)?.itemStyle.opacity;
    expect(opacity('sys:demo/message-queue')).toBeUndefined();
    expect(opacity('ent:cif-0004')).toBeUndefined();
    expect(opacity('cmp:demo/message-queue/broker')).toBeUndefined();
    expect(opacity('sys:demo/cdn')).toBe(0.2);
    const away = links.find((l) => l.source === 'sys:demo/cdn');
    expect(away?.lineStyle.opacity).toBe(0.1);
    expect(away?.label.show).toBe(false);
});

interface Placed {
    x: number;
    y: number;
}

test('names with markup are text in the node and edge tooltips', async ({ page }) => {
    const from = node(`sys:demo/${EVIL}`, 'system', EVIL, null, {
        env: EVIL,
        tier: EVIL,
        flags: [EVIL],
        posture: { score: 41.5, band: EVIL },
        risks: { ...noRisks, open: 1, worst: EVIL, suppressed: 1, suppressed_by: EVIL },
    });
    const to = node('sys:demo/other', 'system', 'other', null);
    const over = { nodes: [from, to], edges: [edge(from.id, 'calls', to.id, EVIL)] };
    await installChartHooks(page);
    await mock(page, [], true, over);
    await page.goto('/starmap');
    await drawn(page);

    const onNode = await showTip(page, 'starmap', { seriesIndex: 0, dataIndex: 0 });
    for (const line of [`(system · ${EVIL})`, `tier: ${EVIL}`, `resilience: 41.5 (${EVIL})`]) {
        await expectShownAsText(page, onNode, line);
    }
    await expectShownAsText(page, onNode, `(worst ${EVIL})`);
    await expectShownAsText(page, onNode, `suppressed by ${EVIL} with no expiry`);

    // An edge has no index to show a tooltip by: the pointer goes onto the
    // straight line ECharts draws between the two nodes, clear of its caption.
    const [a, b] = await page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const option = w.__tumultCharts?.starmap as { series: { data: Placed[] }[] };
        return option.series[0].data.map((d) => [d.x, d.y]);
    });
    const along = (i: number) => a[i] * 0.7 + b[i] * 0.3;
    const onEdge = await showTip(page, 'starmap', { seriesIndex: 0, cell: [along(0), along(1)] });
    await expectShownAsText(page, onEdge, `${from.id} calls sys:demo/other`);
    await expectShownAsText(page, onEdge, `source: ${EVIL}`);
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

test('the star map is drawn in the colours of the light theme on a direct load', async ({
    page,
}) => {
    await page.addInitScript(() => localStorage.setItem('tumult.theme', 'light'));
    await mock(page, []);
    await page.goto('/starmap');
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    await expect.poll(async () => (await mirror(page))?.labelColor).toBe('#5b6772');

    // A node with a gap is filled in the colour of a panel, its border in the colour of text.
    const gap = (await drawn(page)).nodes.find((n) => n.name === 'sys:demo/session-cache');
    expect(gap?.itemStyle).toMatchObject({ color: '#ffffff', borderColor: '#5b6772' });
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
    await expect(drawer.locator('dt', { hasText: 'resilience' })).toHaveCount(1);
    await expect(drawer.locator('dt', { hasText: 'posture' })).toHaveCount(0);
    // The gate verdict names its score and says what the score measures.
    await expect(drawer).toContainText(`${COMPLIANCE_SCORE} 41.5 (poor)`);
    await expect(drawer).toContainText(COMPLIANCE_SCORE_LINE);
    await expect(drawer.getByRole('link', { name: 'Resilience', exact: true })).toHaveAttribute(
        'href',
        '/resilience?env=demo',
    );
    const runs = seen.find((u) => u.pathname === '/api/experiments');
    expect(runs?.searchParams.get('target')).toBe('message-queue');

    // The selected node carries the accent border in the risk lens.
    const selected = (await drawn(page)).nodes.find((n) => n.name === 'sys:demo/message-queue');
    expect(selected?.itemStyle.borderColor).toBe('#5eb1ef');
    // The fill is the colour of its kind under every lens.
    expect(selected?.itemStyle.color).toBe(SYSTEM_FILL);

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
    const canvas = page.locator('[data-chart="starmap"]');
    await expect(canvas).toHaveAttribute('aria-label', /Star map of 14 nodes and 10 edges/);
    await expect(canvas).toHaveAttribute('role', 'img');

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

test('the drawer is a dialog its node names, beside the map and not modal', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap');
    await drawn(page);
    const list = page.getByRole('region', { name: 'nodes' });
    const queue = list.getByRole('button', { name: 'system message-queue' });
    await queue.focus();
    await page.keyboard.press('Enter');

    const drawer = page.getByRole('dialog', { name: 'message-queue' });
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole('heading', { name: 'message-queue' })).toBeFocused();

    // Not modal: the list beside it still takes a click, and the drawer follows.
    const worker = list.getByRole('button', { name: 'system api-worker' });
    await worker.click();
    await expect(page.getByRole('dialog', { name: 'api-worker' })).toBeVisible();

    await page.keyboard.press('Escape');
    await expect(page.getByTestId('starmap-drawer')).toHaveCount(0);
    await expect(worker).toBeFocused();
});

test('the drawer names disciplines as the Resilience page does', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap?sel=sys%3Ademo%2Fmessage-queue');
    const drawer = page.getByTestId('starmap-drawer');
    await expect(drawer).toContainText('robustness / dr: missing');
    await expect(drawer).toContainText('recommended next: robustness / dr');
});

test('a map built from a capped read says it is incomplete', async ({ page }) => {
    await mock(page, [], true, { truncated: true });
    await page.goto('/starmap');
    await drawn(page);
    await expect(page.getByTestId('starmap-legend')).toContainText(
        'incomplete: a source table is longer than its row limit',
    );
});

test('Escape in the filter box leaves the drawer open', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap?sel=sys%3Ademo%2Fmessage-queue');
    const drawer = page.getByTestId('starmap-drawer');
    await expect(drawer).toHaveCount(1);

    const list = page.getByRole('region', { name: 'nodes' });
    const filter = list.getByRole('searchbox', { name: 'Filter nodes' });
    await filter.focus();
    await page.keyboard.type('queue');
    await page.keyboard.press('Escape');
    await expect(drawer).toHaveCount(1);
    await expect(page).toHaveURL(/sel=sys%3Ademo%2Fmessage-queue/);

    // Anywhere else Escape still closes it.
    await list.getByRole('button').first().focus();
    await page.keyboard.press('Escape');
    await expect(drawer).toHaveCount(0);
});

test('the drawer shows the journal duration of a run, like the experiments list', async ({
    page,
}) => {
    await mock(page, []);
    await page.goto('/starmap?range=30d&sel=def%3Aname%2Fqueue-dependency-loss');
    const drawer = page.getByTestId('starmap-drawer');
    await expect(drawer.getByRole('link', { name: 'queue-dependency-loss' })).toHaveCount(1);
    // The run took 2.0s; its trace span (5.4s) also covers the seal wait.
    await expect(drawer).toContainText('2.0s');
    await expect(drawer).not.toContainText('5.4s');
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
    // The daemon is asked for the definition by its whole name.
    const asked = seen.find((u) => u.pathname === '/api/experiments');
    expect(asked?.searchParams.get('range')).toBe('30d');
    expect(asked?.searchParams.get('name')).toBe('queue-dependency-loss');
    expect(asked?.searchParams.get('q')).toBeNull();
});

test('the drawer marks a run that is a journal only', async ({ page }) => {
    await mock(page, []);
    await page.goto('/starmap?range=30d&sel=sys%3Ademo%2Fmessage-queue');
    const rows = page.getByTestId('starmap-drawer').locator('ul.rows li');
    const ofTheRun = (name: string) =>
        rows.filter({ has: page.getByRole('link', { name, exact: true }) });
    await expect(ofTheRun('queue-dependency-loss')).toContainText('journal only');
    await expect(ofTheRun('message-queue-kill')).toHaveCount(1);
    await expect(ofTheRun('message-queue-kill')).not.toContainText('journal only');
});

/** A verified manual restore of `target` in `environment`. */
const restoreOf = (id: string, target: string, environment: string) => ({
    id,
    experiment_name: `${target} restore`,
    exercise_type: 'restore',
    executed_at_ns: NOW_NS - DAY_NS,
    outcome_status: 'passed',
    target_system: target,
    target_environment: environment,
    status: 'verified',
    entered_by: 'alice',
    reviewed_by: 'bob',
});

/** A planned restore of `target` in `environment`, due in a week. */
const planOf = (id: string, target: string, environment: string) => ({
    id,
    name: `${target} yearly restore`,
    exercise_type: 'restore',
    target_system: target,
    target_environment: environment,
    function_id: null,
    scope: null,
    owner: 'alice',
    planned_at_ns: null,
    due_at_ns: NOW_NS + 7 * DAY_NS,
    interval_days: 365,
    rto_target_s: 900,
    rpo_target_s: 60,
    status: 'planned',
    created_by: 'alice',
    created_at_ns: NOW_NS - DAY_NS,
    updated_by: null,
    updated_at_ns: null,
});

test('the drawer opens the Manual page and the DR page on its system', async ({ page }) => {
    await mock(page, []);
    // A route added later answers first: these records for the Manual page and the drawer.
    await page.route('**/api/manual/experiments', (route) =>
        route.fulfill({
            json: {
                records: [
                    restoreOf('m-1', 'message-queue', 'demo'),
                    restoreOf('m-2', 'message-queue', 'staging'),
                    restoreOf('m-3', 'session-cache', 'demo'),
                ],
            },
        }),
    );
    await page.route('**/api/manual/plans', (route) =>
        route.fulfill({
            json: {
                plans: [
                    planOf('p-1', 'message-queue', 'demo'),
                    planOf('p-2', 'session-cache', 'demo'),
                ],
            },
        }),
    );
    await page.goto('/starmap?sel=sys%3Ademo%2Fmessage-queue');
    const drawer = page.getByTestId('starmap-drawer');
    const query = 'target=message-queue&environment=demo';
    await expect(
        drawer.getByRole('link', { name: 'Disaster recovery', exact: true }),
    ).toHaveAttribute('href', `/dr?${query}`);
    await expect(drawer.getByRole('link', { name: 'Manual records', exact: true })).toHaveAttribute(
        'href',
        `/manual?tab=records&${query}`,
    );
    const record = drawer.getByRole('link', { name: 'message-queue restore', exact: true });
    await expect(record).toHaveCount(1);
    await expect(record).toHaveAttribute('href', `/manual?tab=records&${query}`);

    // The Manual page reads the target of its URL as its filter.
    await record.click();
    await expect(page).toHaveURL(/\/manual\?tab=records&target=message-queue&environment=demo$/);
    const rows = page.locator('tr[data-record]');
    await expect(rows).toHaveCount(1);
    await expect(rows).toHaveAttribute('data-record', 'm-1');
    await expect(page.getByTestId('manual-target')).toHaveText(
        'Records and plans of message-queue in demo. Show every target',
    );

    // The plans keep to the same target, as the filter stays in the URL.
    await page.getByRole('tab', { name: 'Plans', exact: true }).click();
    await expect(page).toHaveURL(/\/manual\?tab=plans&target=message-queue&environment=demo$/);
    const plans = page.locator('tr[data-plan]');
    await expect(plans).toHaveCount(1);
    await expect(plans).toHaveAttribute('data-plan', 'p-1');

    // Without the filter every record is listed.
    await page.getByRole('tab', { name: 'All records', exact: true }).click();
    await page.getByRole('link', { name: 'Show every target', exact: true }).click();
    await expect(page).toHaveURL(/\/manual\?tab=records$/);
    await expect(rows).toHaveCount(3);
    await expect(page.getByTestId('manual-target')).toHaveCount(0);
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

import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import {
    COMPLIANCE_SCORE,
    COMPLIANCE_SCORE_LINE,
    RESILIENCE_SCORE,
    RESILIENCE_SCORE_LINE,
} from '../src/lib/score-names';
import { installChartHooks } from './charts';

// The API is mocked; charts render to canvas, so the coverage heatmap is
// asserted through the option mirrored at window.__tumultCharts.

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const risk = (id: string, rank: number, severity: string, title: string, rule: string) => ({
    id,
    rule,
    severity,
    rank,
    title,
    detail: `${title} detail`,
    target: 'database',
    environment: 'demo',
    subject: 'database',
    recommended_kind: rule === 'discipline_gap' ? 'robustness' : null,
    evidence: [],
    suppression: null,
});

const KINDS = ['chaos', 'performance', 'robustness', 'security'];

// The same target tested in two environments is two rows.
const coverageRow = (environment: string, covered: string | null) => ({
    target: 'database',
    environment,
    criticality: null,
    cells: KINDS.map((kind) => ({
        kind,
        status: kind === covered ? 'covered' : 'missing',
        runs: kind === covered ? 2 : 0,
        last_run_ns: kind === covered ? 1 : null,
    })),
});

const coverage = {
    kinds: KINDS,
    targets: [coverageRow('demo', 'chaos'), coverageRow('staging', null)],
    portfolio: KINDS.map((kind) => ({
        kind,
        covered: kind === 'chaos' ? 1 : 0,
        stale: 0,
        missing: kind === 'chaos' ? 0 : 1,
        ratio: kind === 'chaos' ? 1 : 0,
    })),
};

const risks = [
    risk('r_1', 80, 'high', 'Latest chaos run failed', 'latest_run_failed'),
    risk('r_2', 60, 'high', 'No disaster recovery evidence', 'discipline_gap'),
    risk('r_3', 20, 'low', 'Definition never ran', 'never_run_definition'),
];

test.beforeEach(async ({ page }) => {
    // Charts mirror their option only where a test installed the registry.
    await installChartHooks(page);
});

test('resilience opens with ranked risks, coverage gaps and the score', async ({ page }) => {
    const posted: unknown[] = [];
    let suppressed = false;
    await page.route('**/api/**', async (route) => {
        const req = route.request();
        const path = new URL(req.url()).pathname;
        let body: unknown = {};
        let status = 200;
        if (path === '/api/me') body = me;
        else if (path === '/api/posture')
            body = {
                as_of_ns: 1,
                posture: {
                    score: 62.5,
                    band: 'fair',
                    targets: [
                        {
                            target: 'database',
                            environment: 'demo',
                            criticality: null,
                            score: 62.5,
                            band: 'fair',
                            components: {
                                nfr: null,
                                coverage: 14.3,
                                freshness: 98,
                                recovery: null,
                            },
                        },
                    ],
                },
                summary: { total: 3, critical: 0, high: 2, medium: 0, low: 1, suppressed: 0 },
                coverage,
                top_risks: risks,
                nfrs: [
                    {
                        target: 'database',
                        environment: 'demo',
                        experiment: 'pg-failover',
                        nfr_id: 'p99_latency',
                        kind: 'latency',
                        state: 'met',
                        criticality: 'major',
                        threshold: 250,
                        observed: 180,
                        margin: 70,
                    },
                    {
                        target: 'database',
                        environment: 'demo',
                        experiment: 'pg-failover',
                        nfr_id: 'availability',
                        kind: 'availability',
                        state: 'breached',
                        criticality: 'blocking',
                        threshold: 99.9,
                        observed: 99.1,
                        margin: -0.8,
                    },
                ],
            };
        else if (path === '/api/otel/conformance')
            body = {
                as_of_ns: 1,
                range: '24h',
                contract: {
                    target: [
                        'resilience.target.system',
                        'resilience.target.technology',
                        'resilience.target.environment',
                    ],
                    spine: [
                        'ict.schema.version',
                        'ict.entity.id',
                        'ict.entity.tier',
                        'ict.entity.owner',
                    ],
                    recommended: ['resilience.target.component', 'service.version'],
                },
                summary: { services: 2, spine: 1, target: 0, none: 1, malformed_entity_ids: 1 },
                entity_id_pattern: '^[a-z][a-z0-9]*(-[a-z0-9]+)+$',
                malformed_entity_ids: [
                    {
                        service: 'legacy-batch',
                        entity_id: 'CF-0042',
                        spans: 12,
                        last_seen_ns: 1,
                        flagged_at_ingest: true,
                        reason: 'must start with a lower-case letter',
                    },
                ],
                services: [
                    {
                        service: 'checkout',
                        spans: 40,
                        last_seen_ns: 1,
                        level: 'spine',
                        missing: [],
                        keys: [],
                    },
                    {
                        service: 'legacy-batch',
                        spans: 12,
                        last_seen_ns: 1,
                        level: 'none',
                        missing: ['resilience.target.system', 'ict.entity.id'],
                        keys: [],
                    },
                ],
            };
        else if (path === '/api/scores/tree')
            body = {
                name: 'company',
                path: '',
                score: 62.5,
                band: 'fair',
                weight: 1,
                scored: 1,
                expected: 1,
                children: [],
                delta: 1.5,
                sparkline: [
                    [1, 58],
                    [2, 60],
                    [3, 62.5],
                ],
            };
        else if (path === '/api/risks')
            body = suppressed
                ? {
                      as_of_ns: 1,
                      risks: risks.slice(1),
                      suppressed: [
                          {
                              ...risks[0],
                              suppression: {
                                  risk_id: 'r_1',
                                  reason: 'known flake',
                                  actor: 'ops',
                                  at_ns: 1,
                                  expires_at_ns: null,
                              },
                          },
                      ],
                      summary: { total: 2, critical: 0, high: 1, medium: 0, low: 1, suppressed: 1 },
                      coverage,
                  }
                : {
                      as_of_ns: 1,
                      risks,
                      suppressed: [],
                      summary: { total: 3, critical: 0, high: 2, medium: 0, low: 1, suppressed: 0 },
                      coverage,
                  };
        else if (path === '/api/risks/r_1/suppress' && req.method() === 'POST') {
            posted.push(req.postDataJSON());
            suppressed = true;
            status = 201;
            body = {
                id: 's1',
                risk_id: 'r_1',
                reason: 'known flake',
                actor: 'ops',
                at_ns: 1,
                expires_at_ns: null,
                prev_hash: null,
                new_hash: 'h',
            };
        }
        await route.fulfill({
            status,
            contentType: 'application/json',
            body: JSON.stringify(body),
        });
    });

    await page.goto('/resilience');
    await expect(page.getByRole('heading', { level: 1, name: 'Resilience' })).toBeVisible();
    await expect(
        page.getByTestId('posture').getByRole('heading', { name: RESILIENCE_SCORE }),
    ).toBeVisible();
    // Each of the two scores says in one line what it measures.
    await expect(page.getByTestId('posture')).toContainText(RESILIENCE_SCORE_LINE);
    await expect(
        page.getByTestId('trend').getByRole('heading', { name: `${COMPLIANCE_SCORE}, 14d` }),
    ).toBeVisible();
    await expect(page.getByTestId('trend')).toContainText(COMPLIANCE_SCORE_LINE);
    const list = page.getByTestId('risks').locator('ol.risks > li');
    await expect(list).toHaveCount(3);
    await expect(list.nth(0)).toContainText('Latest chaos run failed');
    await expect(list.nth(1)).toContainText('next: run a robustness / dr test');
    await expect(page.getByTestId('posture')).toContainText('62.5');
    await expect(page.getByTestId('posture')).toContainText('fair');

    // Every chart is an image with a name a screen reader can say.
    const named = page.locator('[data-chart]');
    expect(await named.count()).toBeGreaterThanOrEqual(3);
    for (const chart of await named.all()) {
        await expect(chart).toHaveAttribute('role', 'img');
        await expect(chart).toHaveAttribute('aria-label', /\S/);
    }

    // The heatmap carries one cell per target × discipline.
    const cells = await page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.coverage as
            | {
                  series?: { data?: unknown[] }[];
                  xAxis?: { data?: string[]; axisLabel?: { interval?: number } }[];
                  yAxis?: { data?: string[] }[];
              }
            | undefined;
        return {
            count: opt?.series?.[0]?.data?.length ?? -1,
            rows: opt?.yAxis?.[0]?.data,
            columns: opt?.xAxis?.[0]?.data,
            // Every column is named, none is skipped for want of room.
            interval: opt?.xAxis?.[0]?.axisLabel?.interval,
        };
    });
    expect(cells).toEqual({
        count: 8,
        rows: ['database (demo)', 'database (staging)'],
        columns: ['chaos', 'load / performance', 'robustness / dr', 'security'],
        interval: 0,
    });

    // The radar draws one value per discipline on one axis each.
    const radar = await page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.['coverage-radar'] as
            | {
                  series?: { type?: string; data?: { value?: number[] }[] }[];
                  radar?: { indicator?: { name?: string }[] }[];
              }
            | undefined;
        return {
            type: opt?.series?.[0]?.type,
            values: opt?.series?.[0]?.data?.[0]?.value?.length ?? -1,
            axes: opt?.radar?.[0]?.indicator?.map((axis) => axis.name),
        };
    });
    expect(radar).toEqual({
        type: 'radar',
        values: 4,
        axes: ['chaos', 'load / performance', 'robustness / dr', 'security'],
    });

    // The NFR heatmap has one cell per verdict; the trend carries the
    // sparkline, under the name of the score it draws.
    const charts = await page.evaluate((name) => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const nfr = w.__tumultCharts?.nfr as { series?: { data?: unknown[] }[] } | undefined;
        const trend = w.__tumultCharts?.trend as
            { series?: { name?: string; data?: unknown[] }[] } | undefined;
        return {
            nfrCells: nfr?.series?.[0]?.data?.length ?? -1,
            trendPoints: trend?.series?.find((s) => s.name === name)?.data?.length ?? -1,
        };
    }, COMPLIANCE_SCORE);
    expect(charts).toEqual({ nfrCells: 2, trendPoints: 3 });

    // The telemetry contract panel lists each service with its level and gaps.
    const contract = page.getByTestId('conformance');
    await expect(contract.locator('tbody tr')).toHaveCount(2);
    await expect(contract).toContainText('legacy-batch');
    await expect(contract).toContainText('resilience.target.system, ict.entity.id');
    await expect(contract.locator('tbody tr').nth(0)).toContainText('spine');
    // A malformed ict.entity.id is listed as received, never repaired.
    const malformed = page.getByTestId('malformed-ids');
    await expect(malformed).toContainText('1 malformed ict.entity.id');
    await expect(malformed).toContainText('CF-0042');
    await expect(malformed).toContainText('must start with a lower-case letter');

    // Accepting a finding posts the reason and the list refreshes.
    await list.nth(0).getByRole('button', { name: 'accept' }).click();
    await list.nth(0).getByPlaceholder('reason (why this is accepted)').fill('known flake');
    await list.nth(0).getByPlaceholder('your name').fill('ops');
    await list.nth(0).getByRole('button', { name: 'Accept', exact: true }).click();
    await expect(page.getByTestId('risks').locator('ol.risks > li')).toHaveCount(2);
    expect(posted).toEqual([{ reason: 'known flake', expires_in_days: 30, actor: 'ops' }]);
    await page.getByRole('button', { name: '1 suppressed' }).click();
    await expect(page.getByTestId('risks')).toContainText('known flake');
});

// Twelve objectives of one target: too many ids for the default label
// interval of ECharts, which would skip some.
const OBJECTIVES = Array.from({ length: 12 }, (_, i) => `objective-${i + 1}`);

const objective = (nfr_id: string, i: number) => ({
    target: 'database',
    environment: 'demo',
    experiment: 'pg-failover',
    nfr_id,
    kind: 'latency',
    state: i % 3 === 0 ? 'breached' : 'met',
    criticality: 'major',
    threshold: 250,
    observed: 180,
    margin: i % 2 === 0 ? null : 70,
});

interface Axes {
    x?: string;
    y?: string;
    interval?: number;
    slant?: number;
    columns?: string[];
    colours?: string[];
}

/** The axis names of a heatmap, its columns and the colours of its scale. */
const axesOf = (page: Page, id: string) =>
    page.evaluate((chart): Axes | null => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.[chart] as
            | {
                  xAxis?: {
                      name?: string;
                      data?: string[];
                      axisLabel?: { interval?: number; rotate?: number };
                  }[];
                  yAxis?: { name?: string }[];
                  visualMap?: { inRange?: { color?: string[] } }[];
              }
            | undefined;
        if (!opt) return null;
        return {
            x: opt.xAxis?.[0]?.name,
            y: opt.yAxis?.[0]?.name,
            interval: opt.xAxis?.[0]?.axisLabel?.interval,
            slant: opt.xAxis?.[0]?.axisLabel?.rotate,
            columns: opt.xAxis?.[0]?.data,
            colours: opt.visualMap?.[0]?.inRange?.color,
        };
    }, id);

/** The colour of each swatch of the legend of chart `id`, in order. */
const swatches = (page: Page, id: string) =>
    page
        .locator(`[data-legend="${id}"] .swatch`)
        .evaluateAll((all) =>
            all.map((el) => (el as HTMLElement).style.getPropertyValue('--swatch')),
        );

/** A CSS colour the way a canvas reads it back. */
const rgbOf = (page: Page, colour: string) =>
    page.evaluate((value) => {
        const probe = document.createElement('i');
        probe.style.color = value;
        document.body.append(probe);
        const rgb = getComputedStyle(probe).color;
        probe.remove();
        return rgb;
    }, colour);

/**
 * The colour drawn in the middle of column `column` of `columns` of a
 * heatmap with one row: read from the canvas, since the colour of a cell
 * is the one thing its option does not say.
 */
const cellColour = (page: Page, id: string, column: number, columns: number) =>
    page.evaluate(
        ([chart, at, of]) => {
            const w = window as Window & { __tumultCharts?: Record<string, unknown> };
            const opt = w.__tumultCharts?.[chart] as
                | { grid?: { left: number; right: number; top: number; bottom: number }[] }
                | undefined;
            const grid = opt?.grid?.[0];
            if (!grid) return null;
            // ECharts draws on more than one canvas. The cells are on the one
            // that has a pixel at this place.
            const layers = document.querySelectorAll<HTMLCanvasElement>(
                `[data-chart="${chart}"] canvas`,
            );
            for (const canvas of layers) {
                const scale = canvas.width / canvas.clientWidth;
                const x =
                    grid.left + ((at + 0.5) / of) * (canvas.clientWidth - grid.left - grid.right);
                const y = grid.top + (canvas.clientHeight - grid.top - grid.bottom) / 2;
                const [r, g, b, alpha] = canvas
                    .getContext('2d')
                    ?.getImageData(Math.round(x * scale), Math.round(y * scale), 1, 1).data ?? [
                    0, 0, 0, 0,
                ];
                if (alpha === 255) return `rgb(${r}, ${g}, ${b})`;
            }
            return null;
        },
        [id, column, columns] as const,
    );

test('resilience: the charts name their axes and their colours', async ({ page }) => {
    const summary = { total: 0, critical: 0, high: 0, medium: 0, low: 0, suppressed: 0 };
    const bodies: Record<string, unknown> = {
        '/api/me': me,
        '/api/posture': {
            as_of_ns: 1,
            posture: { score: 50, band: 'fair', targets: [] },
            summary,
            coverage,
            top_risks: [],
            nfrs: OBJECTIVES.map(objective),
        },
        '/api/risks': { as_of_ns: 1, risks: [], suppressed: [], summary, coverage },
    };
    await page.route('**/api/**', async (route) => {
        const body = bodies[new URL(route.request().url()).pathname];
        if (body === undefined) await route.fulfill({ status: 404, json: { error: 'not found' } });
        else await route.fulfill({ json: body });
    });
    await page.goto('/resilience');

    await expect.poll(() => axesOf(page, 'coverage')).not.toBeNull();
    const coverageAxes = await axesOf(page, 'coverage');
    expect([coverageAxes?.x, coverageAxes?.y]).toEqual(['discipline', 'target']);
    await expect(page.locator('[data-legend="coverage"] .item')).toHaveText([
        'missing',
        'stale',
        'covered',
    ]);
    expect(await swatches(page, 'coverage')).toEqual(coverageAxes?.colours);
    await expect(page.locator('[data-chart="coverage-radar"]')).toHaveAttribute(
        'aria-label',
        'Evidence coverage per discipline: the share of targets covered, from 0 to 1',
    );

    // Every objective id is a named column, none skipped, and the ids
    // are written at a slant so that one does not run into the next.
    const objectives = await axesOf(page, 'nfr');
    expect(objectives).toMatchObject({
        x: 'objective',
        y: 'target',
        interval: 0,
        slant: 40,
        columns: OBJECTIVES,
    });
    await expect(page.locator('[data-legend="nfr"] .item')).toHaveText([
        'margin on the bad side',
        'margin near zero',
        'margin on the good side',
        'no margin: met counts as 0.5, breached as -0.5, any other verdict as 0',
    ]);
    expect(await swatches(page, 'nfr')).toEqual(objectives?.colours);

    // A cell has the colour of its margin. The second objective has the
    // largest margin on the good side, so its cell has the colour the legend
    // names for that side. The first has no margin, so its cell has not.
    const good = await rgbOf(page, objectives?.colours?.[2] ?? '');
    await expect.poll(() => cellColour(page, 'nfr', 1, OBJECTIVES.length)).toBe(good);
    expect(await cellColour(page, 'nfr', 0, OBJECTIVES.length)).not.toBe(good);
});

test('GIVEN an old /posture link WHEN it opens THEN it lands on /resilience with the query kept', async ({
    page,
}) => {
    // Only the session answers; the page shows its error for the rest.
    await page.route('**/api/**', (route) =>
        new URL(route.request().url()).pathname === '/api/me'
            ? route.fulfill({
                  status: 200,
                  contentType: 'application/json',
                  body: JSON.stringify(me),
              })
            : route.fulfill({ status: 503, contentType: 'application/json', body: '{}' }),
    );
    await page.goto('/posture?env=x');
    await expect(page).toHaveURL(/\/resilience\?env=x$/);
    await expect(page.getByRole('heading', { level: 1, name: 'Resilience' })).toBeVisible();
});

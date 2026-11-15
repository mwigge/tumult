import { expect, test } from '@playwright/test';

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

test('posture opens with ranked risks, coverage gaps and the score', async ({ page }) => {
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

    await page.goto('/posture');
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
            { series?: { data?: unknown[] }[]; yAxis?: { data?: string[] }[] } | undefined;
        return { count: opt?.series?.[0]?.data?.length ?? -1, rows: opt?.yAxis?.[0]?.data };
    });
    expect(cells).toEqual({ count: 8, rows: ['database (demo)', 'database (staging)'] });

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

    // The NFR heatmap has one cell per verdict; the trend carries the sparkline.
    const charts = await page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const nfr = w.__tumultCharts?.nfr as { series?: { data?: unknown[] }[] } | undefined;
        const trend = w.__tumultCharts?.trend as
            { series?: { name?: string; data?: unknown[] }[] } | undefined;
        return {
            nfrCells: nfr?.series?.[0]?.data?.length ?? -1,
            trendPoints: trend?.series?.find((s) => s.name === 'observed')?.data?.length ?? -1,
        };
    });
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

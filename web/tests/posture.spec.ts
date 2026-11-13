import { expect, test } from '@playwright/test';

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
    recommended_kind: rule === 'discipline_gap' ? 'dr' : null,
    evidence: [],
    suppression: null,
});

const coverage = {
    kinds: ['chaos', 'load', 'performance', 'robustness', 'dr', 'security', 'nfr'],
    targets: [
        {
            target: 'database',
            environment: 'demo',
            criticality: null,
            cells: ['chaos', 'load', 'performance', 'robustness', 'dr', 'security', 'nfr'].map(
                (kind) => ({
                    kind,
                    status: kind === 'chaos' ? 'covered' : 'missing',
                    runs: kind === 'chaos' ? 2 : 0,
                    last_run_ns: kind === 'chaos' ? 1 : null,
                }),
            ),
        },
    ],
    portfolio: ['chaos', 'load', 'performance', 'robustness', 'dr', 'security', 'nfr'].map(
        (kind) => ({
            kind,
            covered: kind === 'chaos' ? 1 : 0,
            stale: 0,
            missing: kind === 'chaos' ? 0 : 1,
            ratio: kind === 'chaos' ? 1 : 0,
        }),
    ),
};

const risks = [
    risk('r_1', 80, 'high', 'Latest chaos run failed', 'latest_run_failed'),
    risk('r_2', 60, 'high', 'No disaster recovery evidence', 'discipline_gap'),
    risk('r_3', 20, 'low', 'Definition never ran', 'never_run_definition'),
];

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
    await expect(list.nth(1)).toContainText('next: run a dr test');
    await expect(page.getByTestId('posture')).toContainText('62.5');
    await expect(page.getByTestId('posture')).toContainText('fair');

    // The heatmap carries one cell per target × discipline.
    const cells = await page.evaluate(() => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const opt = w.__tumultCharts?.coverage as { series?: { data?: unknown[] }[] } | undefined;
        return opt?.series?.[0]?.data?.length ?? -1;
    });
    expect(cells).toBe(7);

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

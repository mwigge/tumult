import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { COMPLIANCE_SCORE, COMPLIANCE_SCORE_LINE } from '../src/lib/score-names';
import { EVIL, expectShownAsText, installChartHooks, showTip } from './charts';
import { article24, dora, maturity, tree } from './dora-data';

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
    const drawer = page.getByRole('dialog', { name: 'Requirement dora.art24.1' });
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

test('the evidence drawer opens from the keyboard, and Escape gives the focus back', async ({
    page,
}) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    const opener = page
        .getByTestId('dora-expectations')
        .getByRole('button', { name: 'ev-dora.art24.1' });
    await opener.focus();
    await page.keyboard.press('Enter');

    // The drawer is a dialog that its requirement names, with the focus on Close.
    const drawer = page.getByRole('dialog', { name: 'Requirement dora.art24.1' });
    await expect(drawer).toContainText('payments-api restore');
    await expect(drawer.getByRole('button', { name: 'Close' })).toBeFocused();

    await page.keyboard.press('Escape');
    await expect(page.getByTestId('evidence-drawer')).toHaveCount(0);
    await expect(opener).toBeFocused();
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

test('the tabs and toggles say which view is on and every chart has a name', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    await expect(page.getByTestId('dora-view')).toBeVisible();
    await expect(page.getByRole('tab', { name: 'DORA and maturity' })).toHaveAttribute(
        'aria-selected',
        'true',
    );
    await expect(page.getByRole('tab', { name: 'Org scores' })).toHaveAttribute(
        'aria-selected',
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

/** The colour of each swatch of the legend of chart `id`, in order. */
const swatches = (page: Page, id: string) =>
    page
        .locator(`[data-legend="${id}"] .swatch`)
        .evaluateAll((all) =>
            all
                .map((el) => (el as HTMLElement).style.getPropertyValue('--swatch'))
                .filter((colour) => colour !== ''),
        );

interface Drawn {
    bars?: string;
    barColour?: string;
    level?: string;
    lines?: { name?: string; colour?: string }[];
    fills?: string[];
}

/** The axis names and the colours of the maturity charts and the heatmap. */
const drawn = (page: Page) =>
    page.evaluate((): Drawn => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        type Option = {
            xAxis?: { name?: string }[];
            yAxis?: { name?: string }[];
            series?: {
                name?: string;
                lineStyle?: { color?: string };
                data?: { fill?: string | null; itemStyle?: { color?: string } }[];
            }[];
        };
        const bars = w.__tumultCharts?.['dora-components'] as Option | undefined;
        const history = w.__tumultCharts?.['dora-history'] as Option | undefined;
        const heatmap = w.__tumultCharts?.['dora-heatmap'] as Option | undefined;
        const fills = (heatmap?.series?.[0]?.data ?? []).flatMap((d) => (d.fill ? [d.fill] : []));
        return {
            bars: bars?.xAxis?.[0]?.name,
            barColour: bars?.series?.[0]?.data?.[0]?.itemStyle?.color,
            level: history?.yAxis?.[0]?.name,
            lines: history?.series?.map((s) => ({ name: s.name, colour: s.lineStyle?.color })),
            fills: [...new Set(fills)].sort(),
        };
    });

test('the charts name their axes and say what each colour is', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    await expect.poll(async () => (await drawn(page)).fills?.length).toBe(3);
    const charts = await drawn(page);
    expect(charts).toMatchObject({ bars: 'score (0 to 100)', level: 'level (1 to 5)' });

    const heatmap = page.locator('[data-legend="dora-heatmap"] .item');
    await expect(heatmap).toHaveText([
        'cell = coverage in %',
        'below 50 %',
        'from 50 %',
        'from 80 %',
        'hatched = unknown share',
        '? no source connected',
        '– no events in window',
    ]);
    // The three hues the cells are drawn in, and no other.
    expect([...(await swatches(page, 'dora-heatmap'))].sort()).toEqual(charts.fills);
    await page.getByRole('button', { name: 'Evidence status' }).click();
    await expect(heatmap).toHaveText([
        'cell = evidence status',
        '0 none',
        '1 designed',
        '2 existing',
        '3 operating',
        'hatched = unknown share',
        '? no source connected',
        '– no events in window',
    ]);
    // The cells are drawn in the hues of their rank, each one a swatch.
    await expect.poll(async () => (await drawn(page)).fills).not.toEqual(charts.fills);
    const ranks = await swatches(page, 'dora-heatmap');
    const fills = (await drawn(page)).fills ?? [];
    expect(fills.length).toBe(3);
    expect(fills.filter((fill) => !ranks.includes(fill))).toEqual([]);

    await expect(page.locator('[data-legend="dora-components"] .item')).toHaveText([
        'component score',
        'the component that caps the level',
    ]);
    expect((await swatches(page, 'dora-components'))[0]).toBe(charts.barColour);
    await expect(page.locator('[data-legend="dora-history"] .item')).toHaveText(['Level', 'Index']);
    expect(charts.lines?.map((line) => line.name)).toEqual(['Level', 'Index']);
    expect(await swatches(page, 'dora-history')).toEqual(charts.lines?.map((line) => line.colour));
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

test('an uncapped roll-up heads the node table with its level', async ({ page }) => {
    await mockApi(page, [], { maturity: { rollup_headline: 'level 2 of target 3' } });
    await page.goto('/scores?view=dora');
    const heading = page.getByTestId('dora-rollup').getByRole('heading');
    await expect(heading).toHaveText('Maturity per node — level 2 of target 3');
});

test('listed nodes are never headed as no nodes on record', async ({ page }) => {
    await mockApi(page, [], { maturity: { rollup_headline: null } });
    await page.goto('/scores?view=dora');
    const section = page.getByTestId('dora-rollup');
    await expect(section.locator('tbody tr')).toHaveCount(6);
    await expect(section.getByRole('heading')).toHaveText('Maturity per node');
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

const scenarioBlock = {
    source: 'Commission Delegated Regulation (EU) 2024/1774, Arts. 25, 26, 39 and 40',
    source_url: 'https://eur-lex.europa.eu/eli/reg_del/2024/1774/oj',
    rows: ['rts1774.art39'],
    window_days: 365,
    note: 'A scenario with no test is a gap, not a pass.',
    catalogue: [
        {
            id: 'cyber_attack',
            title: 'A cyber attack',
            cite: 'RTS 2024/1774 Art. 26(2)(a); Art. 39(1)',
        },
        {
            id: 'backup_restore',
            title: 'Restore from backups',
            cite: 'RTS 2024/1774 Art. 25(2)(c); Art. 26(2)(a); Art. 39(2)(g); Art. 40(1)',
        },
    ],
    functions: [
        {
            entity_id: 'cif-0001',
            name: 'Payments',
            tier: 'critical',
            row: 'rts1774.art39',
            coverage: 0.5,
            counts: { passed: 1, failed: 0, overdue: 0, missing: 1 },
            scenarios: [
                {
                    id: 'cyber_attack',
                    state: 'passed',
                    last_tested_at: '2026-11-03',
                    last_test: { id: 'run-cyber', source: 'run', outcome: 'passed' },
                    last_passed_at: '2026-11-03',
                },
                {
                    id: 'backup_restore',
                    state: 'missing',
                    last_tested_at: null,
                    last_test: null,
                    last_passed_at: null,
                },
            ],
        },
    ],
};

test('the plan scenarios of each function show their state and their source', async ({ page }) => {
    await mockApi(page, [], { dora: { scenarios: scenarioBlock } });
    await page.goto('/scores?view=dora');
    const panel = page.getByTestId('dora-scenarios');
    await expect(panel.getByRole('table', { name: 'Plan scenarios by function' })).toBeVisible();
    const cyber = panel.locator('tr[data-scenario="cyber_attack"]');
    await expect(cyber.locator('.badge.ok')).toHaveText('passed');
    await expect(cyber).toContainText('tested 2026-11-03');
    const restore = panel.locator('tr[data-scenario="backup_restore"]');
    await expect(restore.locator('.badge.neutral')).toHaveText('missing');
    await expect(panel.locator('tr[data-counts]')).toContainText('1 of 2');
    await expect(panel.getByRole('link', { name: /2024\/1774/ })).toHaveAttribute(
        'href',
        'https://eur-lex.europa.eu/eli/reg_del/2024/1774/oj',
    );
    await expect(panel).toContainText('A scenario with no test is a gap, not a pass.');
});

test('the plan scenario table of three functions fits a wide screen', async ({ page }) => {
    const [first] = scenarioBlock.functions;
    const functions = [
        ['cif-0001', 'Order processing', 'critical'],
        ['cif-0002', 'Messaging', 'important'],
        ['cif-0003', 'Batch settlement', 'important'],
    ].map(([entity_id, name, tier]) => ({ ...first, entity_id, name, tier }));
    await page.setViewportSize({ width: 1440, height: 900 });
    await mockApi(page, [], { dora: { scenarios: { ...scenarioBlock, functions } } });
    await page.goto('/scores?view=dora');
    const panel = page.getByTestId('dora-scenarios');
    await expect(panel.getByRole('columnheader', { name: /Batch settlement/ })).toBeVisible();
    const fit = await panel.evaluate((box) => {
        const edge = box.getBoundingClientRect();
        return {
            overflow: box.scrollWidth - box.clientWidth,
            cut: [...box.querySelectorAll('th[scope="col"]')]
                .filter((th) => {
                    const cell = th.getBoundingClientRect();
                    return cell.left < edge.left || cell.right > edge.right;
                })
                .map((th) => (th.textContent ?? '').trim()),
        };
    });
    expect
        .soft(fit.overflow, 'the table scrolls sideways by this many pixels')
        .toBeLessThanOrEqual(0);
    expect(fit.cut, 'column headers cut by the edge of the panel').toEqual([]);
});

test('a daemon that sends no plan scenarios shows no scenario panel', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores?view=dora');
    await expect(page.getByTestId('dora-readiness')).toBeVisible();
    await expect(page.getByTestId('dora-scenarios')).toHaveCount(0);
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

test('the charts are drawn in the colours of the light theme on a direct load', async ({
    page,
}) => {
    await page.addInitScript(() => localStorage.setItem('tumult.theme', 'light'));
    await mockApi(page);
    await page.goto('/scores?view=dora');
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
    const colours = () =>
        page.evaluate(() => {
            type Option = {
                tooltip?: { backgroundColor?: string }[];
                series?: {
                    axisLabel?: { color?: string };
                    lineStyle?: { color?: string };
                    data?: { itemStyle?: { color?: string } }[];
                }[];
            };
            const w = window as Window & { __tumultCharts?: Record<string, unknown> };
            const of = (id: string) => w.__tumultCharts?.[id] as Option | undefined;
            return {
                heatmap: of('dora-heatmap')?.tooltip?.[0]?.backgroundColor,
                bar: of('dora-components')?.series?.[0]?.data?.[0]?.itemStyle?.color,
                gauge: of('dora-gauge')?.series?.[0]?.axisLabel?.color,
                history: of('dora-history')?.series?.[0]?.lineStyle?.color,
            };
        });
    await expect
        .poll(colours)
        .toEqual({ heatmap: '#ffffff', bar: '#1f6fb2', gauge: '#5b6772', history: '#1f6fb2' });
});

test('the navigation reaches the DORA view and the org scores stay', async ({ page }) => {
    await mockApi(page);
    await page.goto('/scores');
    await expect(page.getByText('Score — company')).toBeVisible();
    // The org score is the compliance score, and the page says so.
    const meaning = page.getByTestId('score-meaning');
    await expect(meaning).toHaveText(`${COMPLIANCE_SCORE}. ${COMPLIANCE_SCORE_LINE}`);
    await expect(page.getByTestId('dora-view')).toHaveCount(0);
    await page.getByRole('link', { name: 'DORA' }).click();
    await expect(page).toHaveURL(/view=dora/);
    await expect(page.getByTestId('dora-view')).toBeVisible();
    await expect(meaning).toHaveCount(0);
    await page.getByRole('tab', { name: 'Org scores' }).click();
    await expect(page.getByText('Score — company')).toBeVisible();
});

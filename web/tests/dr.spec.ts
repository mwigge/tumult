import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { DR_GUIDE_URL } from '../src/lib/dr';
import { EVIL, expectShownAsText, installChartHooks, showTip } from './charts';
import { DAY_MS, emptyLake, fullLake, serveDr } from './dr-data';

// The API is mocked by a stand-in that filters and pages the tests as
// `GET /api/dr` does; the live test reads the real one. Charts render to
// canvas, so they are asserted through the option mirrored at
// window.__tumultCharts.

const NOW = Date.now();

test.beforeEach(async ({ page }) => {
    await installChartHooks(page);
});

interface Drawn {
    series: {
        name: string;
        data: { value: [number, number] }[];
        markLine?: { data: { yAxis: number; name: string }[] };
        itemStyle?: { color?: string };
    }[];
}

/** What the chart of `kind` of the target `key` was drawn with. */
async function chartOf(page: Page, key: string, kind: 'rto' | 'rpo'): Promise<Drawn> {
    const host = page.locator(`section[data-target="${key}"] [data-chart^="dr-${kind}-"]`);
    const id = await host.getAttribute('data-chart');
    const read = (chart: string) => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        return (w.__tumultCharts?.[chart] ?? { series: [] }) as Drawn;
    };
    await expect
        .poll(async () => (await page.evaluate(read, id ?? '')).series.length)
        .toBeGreaterThan(0);
    return page.evaluate(read, id ?? '');
}

const values = (series: Drawn['series'][number]) => series.data.map((d) => d.value[1]);

/** The text of each cell of the rows of `table`. */
async function rowsOf(page: Page, testid: string): Promise<string[][]> {
    return page
        .locator(`section[aria-labelledby="${testid}"] tbody tr`)
        .evaluateAll((rows) =>
            rows.map((row) =>
                [...row.querySelectorAll('th, td')].map((cell) =>
                    (cell.textContent ?? '').replace(/\s+/g, ' ').trim(),
                ),
            ),
        );
}

test('the page is in the navigation beside the evidence pages and opens from it', async ({
    page,
}) => {
    await serveDr(page, fullLake(NOW), NOW);
    await page.goto('/');
    await expect(page.getByRole('link', { name: 'DORA' })).toBeVisible();
    // Each entry is its icon and then its name.
    const names = await page
        .locator('nav.nav a')
        .evaluateAll((links) => links.map((a) => (a.lastChild?.textContent ?? '').trim()));
    expect(names.indexOf('Disaster recovery')).toBe(names.indexOf('DORA') + 1);

    await page.getByRole('link', { name: 'Disaster recovery' }).click();
    await expect(page).toHaveURL(/\/dr$/);
    await expect(page.locator('.page-head h1')).toHaveText('Disaster recovery');
    await expect(page.getByRole('link', { name: 'Disaster recovery' })).toHaveClass(/active/);
});

test('an empty lake says that there is no DR test yet and what to do next', async ({ page }) => {
    await serveDr(page, emptyLake(), NOW);
    await page.goto('/dr');
    await expect(page.getByText('No DR tests yet.')).toBeVisible();
    await expect(page.getByRole('link', { name: 'the DR guide' })).toHaveAttribute(
        'href',
        DR_GUIDE_URL,
    );
    await expect(page.getByRole('link', { name: 'Manual page' })).toHaveAttribute(
        'href',
        '/manual',
    );
    await expect(page.getByRole('link', { name: 'Plans' })).toHaveAttribute(
        'href',
        '/manual?tab=plans',
    );
    await expect(page.getByRole('heading', { name: 'Due register' })).toHaveCount(0);
});

test('the register shows each target in its state with its last test, next due date and plan', async ({
    page,
}) => {
    await serveDr(page, fullLake(NOW), NOW);
    await page.goto('/dr');
    await expect(page.getByTestId('dr-register-summary')).toHaveText(
        '7 targets: 2 ok, 1 due, 2 overdue, 2 never tested',
    );
    const rows = await rowsOf(page, 'dr-register');
    expect(rows.map((r) => [r[0], r[1]])).toEqual([
        ['archive · prod', 'never tested'],
        ['billing · prod', 'overdue'],
        ['catalog · prod', 'due'],
        ['ledger · prod', 'ok'],
        ['orders · prod', 'ok'],
        ['payments-db · prod', 'overdue'],
        ['search · staging', 'never tested'],
    ]);
    const payments = page.locator('tr[data-target="payments-db/prod"]');
    await expect(payments.locator('td').nth(1)).toContainText('automated not recovered');
    await expect(payments.locator('td').nth(3)).toHaveText('every 90 days from its plan');
    await expect(payments.locator('td').nth(5)).toContainText(
        'at least 30.1 s, not recovered target 10 s',
    );
    await expect(payments.getByRole('link', { name: /^due / })).toHaveAttribute(
        'href',
        '/manual?tab=plans',
    );
    const catalog = page.locator('tr[data-target="catalog/prod"]');
    await expect(catalog.locator('td').nth(3)).toHaveText('every 365 days by default');
    await expect(catalog.locator('td').nth(1).getByRole('link')).toHaveAttribute(
        'href',
        '/manual?tab=records',
    );
    await expect(catalog.locator('td').nth(5)).toHaveText('5 min target 15 min');
    const archive = page.locator('tr[data-target="archive/prod"]');
    await expect(archive.locator('td').nth(1)).toHaveText('no DR test yet');
    await expect(archive.locator('td').nth(2)).toHaveText('test now');
});

test('the RTO of a target is a line with its target as a rule, a manual point and an unrecovered point', async ({
    page,
}) => {
    await serveDr(page, fullLake(NOW), NOW);
    await page.goto('/dr');
    const rto = await chartOf(page, 'ledger/prod', 'rto');
    expect(rto.series.map((s) => s.name)).toEqual([
        'measured',
        'manual',
        'not recovered, at least',
    ]);
    expect(values(rto.series[0])).toEqual([55, 42, 50, 38]);
    expect(values(rto.series[1])).toEqual([55, 50]);
    expect(values(rto.series[2])).toEqual([75]);
    expect(rto.series[0].markLine?.data.map((d) => [d.yAxis, d.name])).toEqual([
        [60, 'target 60 s'],
    ]);
    const rpo = await chartOf(page, 'ledger/prod', 'rpo');
    expect(rpo.series.map((s) => [s.name, values(s)])).toEqual([
        ['measured', [20, 9, 12, 8]],
        ['manual', [20, 12]],
    ]);
    expect(rpo.series[0].markLine?.data.map((d) => d.yAxis)).toEqual([30]);
    await expect(page.locator('section[data-target="ledger/prod"] [role="img"]')).toHaveCount(2);
    await expect(
        page
            .locator('section[data-target="search/staging"]')
            .getByText('No RTO measured in this period.'),
    ).toBeVisible();
});

test('the tooltip of a point names its test as text', async ({ page }) => {
    const lake = fullLake(NOW);
    const drill = lake.tests.find((t) => t.test_id === 'manual:01LEDGERDRILL');
    if (!drill) throw new Error('the lake has the ledger drill');
    drill.experiment_name = EVIL;
    await serveDr(page, lake, NOW);
    await page.goto('/dr');
    const id = await page
        .locator('section[data-target="ledger/prod"] [data-chart^="dr-rto-"]')
        .getAttribute('data-chart');
    const tip = await showTip(page, id ?? '', { seriesIndex: 1, dataIndex: 1 });
    await expectShownAsText(page, tip);
    await expect(tip).toContainText('RTO: 50 s');
    await expect(tip).toContainText('manual drill');
});

test('the charts follow the theme', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('tumult.theme', 'light'));
    await serveDr(page, fullLake(NOW), NOW);
    await page.goto('/dr');
    const light = await chartOf(page, 'ledger/prod', 'rto');
    expect(light.series[2].itemStyle?.color).toBe('#c0392b');
    // The theme goes from light to auto, which is light here, and then to dark.
    await page.getByRole('button', { name: /light/ }).click();
    await page.getByRole('button', { name: /auto/ }).click();
    await expect(page.getByRole('button', { name: /dark/ })).toBeVisible();
    await expect
        .poll(async () => (await chartOf(page, 'ledger/prod', 'rto')).series[2].itemStyle?.color)
        .toBe('#ef4444');
});

test('the tests of the period come newest first and page with the cursor of the API', async ({
    page,
}) => {
    const lake = fullLake(NOW);
    const seen = await serveDr(page, lake, NOW);
    await page.goto('/dr');
    const pager = page.getByRole('navigation', { name: 'Pages of DR tests' });
    await expect(page.getByTestId('dr-tests-summary')).toContainText(
        '45 DR tests in the period: 42 automated, 3 manual',
    );
    await expect(pager).toContainText('Page 1: tests 1 to 20 of 45');
    const ids = () =>
        page
            .locator('section[aria-labelledby="dr-tests"] tbody tr')
            .evaluateAll((rows) => rows.map((r) => r.getAttribute('data-test')));
    expect(await ids()).toEqual(lake.tests.slice(0, 20).map((t) => t.test_id));
    await expect(pager.getByRole('button', { name: 'Previous' })).toBeDisabled();

    await pager.getByRole('button', { name: 'Next' }).click();
    await expect(pager).toContainText('Page 2: tests 21 to 40 of 45');
    expect(await ids()).toEqual(lake.tests.slice(20, 40).map((t) => t.test_id));
    const turned = seen.at(-1);
    expect([turned?.searchParams.get('cursor'), turned?.searchParams.get('limit')]).toEqual([
        '20',
        '20',
    ]);

    await pager.getByRole('button', { name: 'Next' }).click();
    await expect(pager).toContainText('Page 3: tests 41 to 45 of 45');
    await expect(pager.getByRole('button', { name: 'Next' })).toBeDisabled();
    await pager.getByRole('button', { name: 'Previous' }).click();
    await expect(pager).toContainText('Page 2: tests 21 to 40 of 45');
    expect(seen.at(-1)?.searchParams.get('cursor')).toBe('20');
    await pager.getByRole('button', { name: 'Previous' }).click();
    await expect(pager).toContainText('Page 1: tests 1 to 20 of 45');
    expect(seen.at(-1)?.searchParams.has('cursor')).toBe(false);
});

test('each test links to the page of its run or to the manual records', async ({ page }) => {
    await serveDr(page, fullLake(NOW), NOW);
    await page.goto('/dr?target=ledger');
    const table = page.locator('section[aria-labelledby="dr-tests"]');
    await expect(table.getByRole('link', { name: 'ledger failover drill' })).toHaveAttribute(
        'href',
        '/manual?tab=records',
    );
    const run = table.locator('tr[data-test="automated:ledger-dr-120"]');
    await expect(run).toContainText('at least 75 s, not recovered');
    await run.getByRole('link', { name: 'ledger failover' }).click();
    await expect(page).toHaveURL(/\/experiments\/ledger-dr-120$/);
});

test('a filter goes in the URL and in the request, and a URL with filters opens with them', async ({
    page,
}) => {
    const seen = await serveDr(page, fullLake(NOW), NOW);
    await page.goto('/dr');
    await expect(page.getByLabel('Target')).toContainText('payments-db');
    await page.getByLabel('Target').selectOption('ledger');
    await expect(page).toHaveURL(/\/dr\?target=ledger$/);
    await expect(page.locator('section[aria-labelledby="dr-register"] tbody tr')).toHaveCount(1);
    await expect(page.locator('section[aria-labelledby="dr-tests"] tbody tr')).toHaveCount(5);
    expect(seen.at(-1)?.searchParams.get('target')).toBe('ledger');

    await page.getByRole('button', { name: '90 days' }).click();
    await expect(page).toHaveURL(/\/dr\?target=ledger&period=90d$/);
    await expect(page.getByRole('button', { name: '90 days' })).toHaveAttribute(
        'aria-pressed',
        'true',
    );
    await expect(page.locator('section[aria-labelledby="dr-tests"] tbody tr')).toHaveCount(2);
    const from = Number(seen.at(-1)?.searchParams.get('from')) / 1e6;
    expect(Math.abs(from - (Date.now() - 90 * DAY_MS))).toBeLessThan(120_000);

    await page.goto('/dr?target=ledger&environment=prod&period=3y');
    await expect(page.getByLabel('Target')).toHaveValue('ledger');
    await expect(page.getByLabel('Environment')).toHaveValue('prod');
    await expect(page.getByRole('button', { name: '3 years' })).toHaveAttribute(
        'aria-pressed',
        'true',
    );
    const opened = seen.at(-1);
    expect(opened?.searchParams.get('environment')).toBe('prod');
    const start = Number(opened?.searchParams.get('from')) / 1e6;
    expect(Math.abs(start - (Date.now() - 1095 * DAY_MS))).toBeLessThan(120_000);
});

test('a filter that keeps nothing says so and offers every target', async ({ page }) => {
    await serveDr(page, fullLake(NOW), NOW);
    await page.goto('/dr?environment=dev');
    await expect(page.getByText('No target and no DR test match these filters.')).toBeVisible();
    await page.getByRole('link', { name: 'Show every target' }).click();
    await expect(page).toHaveURL(/\/dr$/);
    await expect(page.locator('section[aria-labelledby="dr-register"] tbody tr')).toHaveCount(7);
});

test('a refusal of the API shows its reason', async ({ page }) => {
    await serveDr(page, fullLake(NOW), NOW, 500);
    await page.goto('/dr');
    await expect(page.getByText('Failed to load the DR view: the lake is busy')).toBeVisible();
});

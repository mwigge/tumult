// The HTML report of `tumult report`, opened from its file as a reader
// opens it: the real binary writes it from a recorded journal, and a
// browser without a network draws it.
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';
import { DARK, LIGHT } from '../src/lib/chart-theme';

const RECORDED = fileURLToPath(
    new URL('../../examples/recorded/api-saturation.journal.toon', import.meta.url),
);
const CHARTS = [
    'chart-timeline',
    'chart-load-latency',
    'chart-load-throughput',
    'chart-probes',
    'chart-headroom',
];

function tumult(): string {
    const binary = process.env.TUMULT_BIN;
    if (!binary) throw new Error('TUMULT_BIN must name the tumult binary that writes the reports');
    return binary;
}

// The URL of the report the binary writes for `journal`, in a home of its own.
function reportOf(journal: string): string {
    const home = mkdtempSync(join(tmpdir(), 'tumult-report-'));
    const report = join(home, 'report.html');
    execFileSync(tumult(), ['report', journal, '--format', 'html', '--output', report], {
        cwd: home,
        env: {
            PATH: process.env.PATH,
            HOME: home,
            TUMULT_LAKE_PATH: join(home, 'lake.duckdb'),
            SOURCE_DATE_EPOCH: '1794873600',
        },
    });
    return pathToFileURL(report).href;
}

// The recorded journal with `changes` made to its text, as a file.
function journalWith(changes: [string, string][]): string {
    let journal = readFileSync(RECORDED, 'utf8');
    for (const [from, to] of changes) {
        expect(journal, `the recorded journal has ${from}`).toContain(from);
        journal = journal.replaceAll(from, to);
    }
    const path = join(mkdtempSync(join(tmpdir(), 'tumult-journal-')), 'journal.toon');
    writeFileSync(path, journal);
    return path;
}

// What the chart `id` is drawn with: the markup of its SVG.
async function drawn(page: Page, id: string): Promise<string> {
    const svg = page.locator(`#${id} svg`);
    await expect(svg).toBeVisible();
    return svg.evaluate((element) => element.outerHTML);
}

// The widest mark of `colour` in the chart `id`: the bar of its longest step.
async function widestMark(page: Page, id: string, colour: string): Promise<Locator> {
    const marks = page.locator(`#${id} svg path[fill="${colour}"]`);
    await expect(marks.first()).toBeVisible();
    let widest = marks.first();
    let width = 0;
    for (const mark of await marks.all()) {
        const box = await mark.boundingBox();
        if (box === null || box.width <= width) continue;
        widest = mark;
        width = box.width;
    }
    return widest;
}

test('a report draws every chart of its run without a network', async ({ page, context }) => {
    const report = reportOf(RECORDED);
    const requests: string[] = [];
    page.on('request', (request) => requests.push(request.url()));
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await context.setOffline(true);

    await page.goto(report);

    for (const id of CHARTS) {
        const svg = await drawn(page, id);
        // A chart that is drawn has marks: paths for its bars or lines.
        expect(svg, id).toContain('<path');
        expect(svg, id).not.toContain('token:');
    }
    expect(requests).toEqual([report]);
    expect(errors).toEqual([]);
});

test('a chart is as high as its report says', async ({ page }) => {
    await page.goto(reportOf(RECORDED));

    for (const id of CHARTS) {
        const host = page.locator(`#${id}`);
        const height = Number(await host.getAttribute('data-height'));
        expect(height, id).toBeGreaterThan(90);
        expect((await host.boundingBox())?.height, id).toBe(height);
    }
});

test('the charts have the colours the browser asks for', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' });
    await page.goto(reportOf(RECORDED));

    // Every step of the recorded run succeeded, and its load is drawn too.
    const light = await drawn(page, 'chart-timeline');
    expect(light).toContain(LIGHT.ok);
    expect(light).toContain(LIGHT.accent);
    expect(light).not.toContain(DARK.ok);

    // A reader who changes the setting sees the charts follow it.
    await page.emulateMedia({ colorScheme: 'dark' });
    await expect.poll(async () => drawn(page, 'chart-timeline')).toContain(DARK.ok);
    const dark = await drawn(page, 'chart-timeline');
    expect(dark).toContain(DARK.accent);
    expect(dark).not.toContain(LIGHT.ok);
    expect(await drawn(page, 'chart-load-throughput')).toContain(DARK.fail);
});

test('a bar of the timeline says what it shows when the reader points at it', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' });
    await page.goto(reportOf(RECORDED));

    // The step that ran for the whole run has the longest bar.
    await (await widestMark(page, 'chart-timeline', LIGHT.ok)).hover();

    await expect(page.locator('#chart-timeline')).toContainText(
        'cpu-stress: succeeded, began at +8 ms, took 2 min 3 s',
    );
});

test('a text of a journal is text in a chart, never markup', async ({ page }) => {
    const step = '<img src=x onerror=window.pwned=1>';
    const probe = '<img src=y onerror=window.pwned=2>';
    const journal = journalWith([
        ['name: "cpu-stress"', `name: "${step}"`],
        ['health,during,', `"${probe}",during,`],
    ]);
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.emulateMedia({ colorScheme: 'light' });
    await page.goto(reportOf(journal));

    // The names are drawn as they are written.
    expect(await drawn(page, 'chart-timeline')).toContain('&lt;img src=x');
    expect(await drawn(page, 'chart-probes')).toContain('&lt;img src=y');

    // The tooltip of a bar names the step, the tooltip of a time the probes.
    await (await widestMark(page, 'chart-timeline', LIGHT.ok)).hover();
    await expect(page.locator('#chart-timeline')).toContainText(`${step}: succeeded`);
    const probes = page.locator('#chart-probes');
    const lines = (await probes.boundingBox())!;
    await page.mouse.move(lines.x + lines.width / 2, lines.y + lines.height / 2);
    await expect(probes).toContainText(probe);

    await expect(page.locator('img')).toHaveCount(0);
    expect(await page.evaluate(() => (window as { pwned?: number }).pwned)).toBeUndefined();
    expect(errors).toEqual([]);
});

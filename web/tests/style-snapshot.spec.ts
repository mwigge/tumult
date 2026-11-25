import { expect, test } from '@playwright/test';

import {
    compare,
    fileOf,
    readBaseline,
    removeBaseline,
    report,
    settledStyles,
    staleBaselines,
    UPDATE,
    writeBaseline,
} from './style-snapshot';
import type { Theme } from './style-snapshot';
import { NOW_MS, PAGES, serve, STATES } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// Every page looks as it does on main: the computed styles of each visible
// element of each page, in both themes, on a wide screen and on a phone,
// are those of its baseline in tests/style-snapshots/. So do the states
// that a click opens, such as an open row or a drawer. A change on purpose
// writes the baselines again: STYLE_SNAPSHOT_UPDATE=1.

const THEMES: Theme[] = ['light', 'dark'];
const SIZES = [
    { width: 1440, height: 900 },
    { width: 390, height: 844 },
];
// A state of a page is read on the wide screen only, to keep the spec short.
const STATE_SIZES = SIZES.slice(0, 1);
const sizesOf = (target: SnapshotPage) => (target.shows ? STATE_SIZES : SIZES);

test.use({ locale: 'en-US', timezoneId: 'UTC' });

test('a changed style reads as the element, the property and the values before and after', () => {
    const before = {
        'div.panel': { color: 'rgb(1, 2, 3)', display: 'block' },
        'span.old': { display: 'inline' },
    };
    const after = {
        'div.panel': { color: 'rgb(9, 9, 9)', display: 'block', padding: '4px' },
        'b.new': { display: 'inline' },
    };
    expect(report('/runs, light theme, 1440 px wide', compare(before, after))).toBe(
        [
            '/runs, light theme, 1440 px wide: 4 style changes',
            '  span.old',
            '    element: shown -> gone',
            '  div.panel',
            '    color: rgb(1, 2, 3) -> rgb(9, 9, 9)',
            '    padding: (default) -> 4px',
            '  b.new',
            '    element: absent -> shown',
            'When the change is on purpose, write the baseline again: STYLE_SNAPSHOT_UPDATE=1.',
        ].join('\n'),
    );
});

test('a long list of changes is cut and counted', () => {
    const before = Object.fromEntries(
        Array.from({ length: 40 }, (_, i) => [`li.row #${i}`, { display: 'block' }]),
    );
    const lines = report('/runs, dark theme, 390 px wide', compare(before, {})).split('\n');
    expect(lines[0]).toBe('/runs, dark theme, 390 px wide: 40 style changes');
    expect(lines.at(-2)).toBe('  and 10 more changes');
});

for (const target of [...PAGES, ...STATES]) {
    const title = target.shows ? `${target.path} with ${target.shows}` : target.path;
    for (const theme of THEMES) {
        for (const size of sizesOf(target)) {
            test(`${title} looks as it does on main, ${theme} theme, ${size.width} px wide`, async ({
                page,
            }) => {
                const errors: string[] = [];
                page.on('pageerror', (error) => errors.push(error.message));
                await page.clock.setFixedTime(NOW_MS);
                await page.setViewportSize(size);
                await page.addInitScript(
                    (saved) => localStorage.setItem('tumult.theme', saved),
                    theme,
                );
                const missing = await serve(page, target);
                await page.goto(target.path);
                await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
                if (target.heading)
                    await expect(
                        page.getByRole('heading', { level: 1, name: target.heading, exact: true }),
                    ).toBeVisible();
                await page.waitForLoadState('networkidle');
                if (target.open) {
                    await target.open(page);
                    await page.waitForLoadState('networkidle');
                }
                const elements = await settledStyles(page);
                expect(missing, 'every API call of the page has an answer').toEqual([]);
                expect(errors, 'the page throws no error').toEqual([]);

                const file = fileOf(target.name, theme, size.width);
                const snapshot = { page: target.name, path: target.path, theme, ...size, elements };
                if (UPDATE) {
                    writeBaseline(file, snapshot);
                    return;
                }
                const baseline = readBaseline(file);
                expect(
                    baseline,
                    `no baseline ${file}: write it with STYLE_SNAPSHOT_UPDATE=1`,
                ).not.toBeNull();
                const changes = compare(baseline?.elements ?? {}, elements);
                const where = `${title}, ${theme} theme, ${size.width} px wide`;
                expect(changes.length, report(where, changes)).toBe(0);
            });
        }
    }
}

test('every baseline is of a page, theme and width of this spec', () => {
    const expected = [...PAGES, ...STATES].flatMap((target) =>
        THEMES.flatMap((theme) =>
            sizesOf(target).map((size) => fileOf(target.name, theme, size.width)),
        ),
    );
    const stale = staleBaselines(expected);
    if (UPDATE) {
        stale.forEach(removeBaseline);
        return;
    }
    expect(stale, 'remove them, or run with STYLE_SNAPSHOT_UPDATE=1').toEqual([]);
});

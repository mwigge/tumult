import { expect, test } from '@playwright/test';

import { show } from './keyboard';
import { labels } from './style-snapshot';
import { PAGES, STATES } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// Every page names itself in the title of the browser tab, and every
// field on it has a name that a screen reader says: its label, or an
// aria-label where the page has no room for a label. The names are read
// from the accessibility tree of the browser.

test.use({ locale: 'en-US', timezoneId: 'UTC' });

const FIELDS = ['input', 'select', 'textarea'];

/** The title of the tab of a page: its heading, or what the login page asks for. */
const titleOf = (target: SnapshotPage) =>
    `${target.name === 'login' ? 'Sign in' : target.heading} · Krönika`;

for (const target of [...PAGES, ...STATES]) {
    const title = target.shows ? `${target.path} with ${target.shows}` : target.path;

    test(`${title} names itself in the title of the tab and names every field`, async ({
        page,
    }) => {
        await show(page, target);

        await expect.soft(page).toHaveTitle(titleOf(target));

        // A field the tree ignores, such as a hidden one, has no role.
        const fields = (await labels(page)).filter((label) => FIELDS.includes(label.tag));
        const shown = await page
            .locator(FIELDS.join(', '))
            .evaluateAll((els) => els.map((el) => el.outerHTML.slice(0, 90)));
        const unnamed = fields.flatMap((field, i) =>
            field.role !== '' && field.name === '' ? [shown[i]] : [],
        );
        expect(unnamed, `${unnamed.length} fields with no name`).toEqual([]);
    });
}

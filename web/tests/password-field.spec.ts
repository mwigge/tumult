import { expect, test } from '@playwright/test';

import { NOW_MS, PAGES, serve } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// A password field is a field like the others: the shared rule of theme.css
// gives it the same edge, corners, padding, text size and height as the
// text field beside it.

const users = PAGES.find((target) => target.name === 'users') as SnapshotPage;

const LOOK = [
    'border-top-width',
    'border-top-style',
    'border-top-color',
    'border-radius',
    'padding-top',
    'padding-left',
    'font-size',
    'background-color',
    'height',
] as const;

test('the password field of a new user looks like the username field', async ({ page }) => {
    await page.clock.setFixedTime(NOW_MS);
    await serve(page, users);
    await page.goto(users.path);
    const form = page.locator('form.create');
    const look = (name: string) =>
        form
            .getByLabel(name)
            .evaluate(
                (el, props) =>
                    props.map((p) => `${p}: ${getComputedStyle(el).getPropertyValue(p)}`),
                [...LOOK],
            );
    expect(await look('Password (optional)')).toEqual(await look('Username'));
});

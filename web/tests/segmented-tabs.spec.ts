import { expect, test } from '@playwright/test';

import { NOW_MS, PAGES, serve, STATES } from './style-snapshot-pages';
import type { SnapshotPage } from './style-snapshot-pages';

// A segmented control says which of its buttons is pressed. The Manual
// page has real tabs: the arrow keys, Home and End move to a tab and
// select it, only the chosen tab is in the tab order, and the URL holds
// the tab.

const runs = PAGES.find((target) => target.name === 'runs') as SnapshotPage;
const manual = STATES.find((target) => target.name === 'manual-records') as SnapshotPage;

test.beforeEach(async ({ page }) => {
    await page.clock.setFixedTime(NOW_MS);
});

test('a segmented control is a named group, and a click presses the button it chose', async ({
    page,
}) => {
    await serve(page, runs);
    await page.goto('/runs');
    const group = page.getByRole('group', { name: 'run state' });
    const all = group.getByRole('button', { name: 'all', exact: true });
    const passed = group.getByRole('button', { name: 'passed', exact: true });
    await expect(all).toHaveAttribute('aria-pressed', 'true');
    await expect(passed).toHaveAttribute('aria-pressed', 'false');
    for (const button of await group.getByRole('button').all())
        await expect(button).toHaveAttribute('aria-pressed', /^(true|false)$/);

    await passed.click();
    await expect(passed).toHaveAttribute('aria-pressed', 'true');
    await expect(passed).toHaveClass('active');
    await expect(all).toHaveAttribute('aria-pressed', 'false');
});

test('the Manual tabs move and select with the arrow keys, Home and End, and the URL follows', async ({
    page,
}) => {
    await serve(page, { ...manual, path: '/manual' });
    await page.goto('/manual');
    const list = page.getByRole('tablist', { name: 'manual sections' });
    const tab = (name: string) => list.getByRole('tab', { name, exact: true });
    await expect(tab('New entry')).toHaveAttribute('aria-selected', 'true');

    await page.getByPlaceholder('your name').focus();
    await page.keyboard.press('Tab');
    await expect(tab('New entry')).toBeFocused();

    await page.keyboard.press('ArrowRight');
    await expect(tab('Plans')).toBeFocused();
    await expect(tab('Plans')).toHaveAttribute('aria-selected', 'true');
    await expect(tab('New entry')).toHaveAttribute('aria-selected', 'false');
    await expect(page).toHaveURL(/\/manual\?tab=plans$/);
    await expect(page.getByRole('tabpanel', { name: 'Plans' })).toBeVisible();

    await page.keyboard.press('End');
    await expect(tab('Import')).toBeFocused();
    await expect(page).toHaveURL(/\/manual\?tab=import$/);

    await page.keyboard.press('ArrowRight');
    await expect(tab('New entry')).toBeFocused();
    await expect(page).toHaveURL(/\/manual\?tab=entry$/);

    await page.keyboard.press('ArrowLeft');
    await expect(tab('Import')).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('tabpanel', { name: 'Import' })).toBeVisible();

    await page.keyboard.press('Home');
    await expect(tab('New entry')).toBeFocused();
    await expect(tab('New entry')).toHaveAttribute('aria-selected', 'true');
    await expect(page).toHaveURL(/\/manual\?tab=entry$/);

    // Only the chosen tab is in the tab order: Tab goes on to the panel.
    await page.keyboard.press('Tab');
    await expect(page.getByRole('tabpanel', { name: 'New entry' })).toBeFocused();
});

test('a Manual tab opened from its URL is the selected tab', async ({ page }) => {
    await serve(page, manual);
    await page.goto('/manual?tab=records');
    const list = page.getByRole('tablist', { name: 'manual sections' });
    await expect(list.getByRole('tab', { name: 'All records' })).toHaveAttribute(
        'aria-selected',
        'true',
    );
    await expect(list.getByRole('tab', { name: 'All records' })).toHaveAttribute('tabindex', '0');
    await expect(list.getByRole('tab', { name: 'Plans' })).toHaveAttribute('tabindex', '-1');
    await expect(page.getByRole('tabpanel', { name: 'All records' })).toBeVisible();
});

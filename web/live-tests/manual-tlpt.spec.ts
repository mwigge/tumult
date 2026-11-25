// A TLPT on the Manual page of a real daemon: the record is refused while
// it cites a requirement id the registry does not know, and the page shows
// the nearest id. Then it is recorded with its tester, its attestation and
// test summary report, and a remediation plan whose item has a severity
// and the run that retested it. It is signed off, and its audit trail
// verifies with the new fields sealed.
import { expect, test, type Page } from '@playwright/test';

import { startDaemon } from './daemon';
import type { Daemon } from './daemon';

const NAME = 'payments TLPT';
const DAY_MS = 86_400_000;

/** A local time as the value of a date or datetime-local field. */
function local(ms: number, length: 10 | 16): string {
    const date = new Date(ms);
    const shifted = new Date(ms - date.getTimezoneOffset() * 60_000);
    return shifted.toISOString().slice(0, length);
}

let daemon: Daemon;

test.beforeAll(async () => {
    daemon = await startDaemon();
});

test.afterAll(() => {
    daemon?.stop();
});

async function actAs(page: Page, name: string) {
    await page.getByPlaceholder('your name').fill(name);
}

test('a TLPT is recorded with its remediation plan and a retest, and signed off', async ({
    page,
}) => {
    test.setTimeout(120_000);
    await page.goto(`${daemon.base}/manual`);
    await actAs(page, 'alice');

    await test.step('a requirement id the registry does not know is refused', async () => {
        await page.getByLabel('Experiment name').fill(NAME);
        await page.getByLabel('Exercise type').selectOption('tlpt');
        await page.getByLabel('Executed at').fill(local(Date.now() - 30 * DAY_MS, 16));
        await page.getByLabel('Target system').fill('payments');
        await page.getByLabel('Target environment').fill('production');
        await page
            .getByLabel('Hypothesis', { exact: true })
            .fill('A red team cannot reach the ledger');
        await page
            .getByLabel('Method', { exact: true })
            .fill('Threat-led test of the payments chain');
        await page.getByLabel('Outcome').selectOption('partial');
        await page
            .getByLabel('Findings', { exact: true })
            .fill('The red team reached the ledger by a stale VPN account');
        await page.getByLabel('Findings severity').selectOption('high');
        await page.getByLabel('Framework refs (comma-sep)').fill('dora.art26.2');
        await page.getByLabel('Attestation text').fill('I attest this is the test as run.');
        await page.getByRole('button', { name: 'Save draft' }).click();
        await expect(
            page.getByText(
                "framework_refs: 'dora.art26.2' is not a requirement id of the DORA registry; did you mean 'dora.art26.1'?",
            ),
        ).toBeVisible();
    });

    await test.step('the tester, the TLPT block and the remediation plan are recorded', async () => {
        await page.getByLabel('Framework refs (comma-sep)').fill('dora.art26.1');
        await page.getByLabel('Tester', { exact: true }).fill('Red Team Ltd');
        await page.getByLabel('Tester kind').selectOption('external');
        await page.getByLabel('Attestation reference').fill('TLPT-ATT-2026-07');
        await page.getByLabel('Attestation date').fill(local(Date.now() - DAY_MS, 10));
        await page.getByLabel('Test summary report reference').fill('TSR-2026-07');
        await expect(page.getByRole('heading', { name: 'Remediation plan' })).toBeVisible();
        await page.getByRole('button', { name: 'Add action' }).click();
        await page.getByLabel('Action 1 title').fill('Remove stale VPN accounts');
        await page.getByLabel('Action 1 owner').fill('carol');
        await page.getByLabel('Action 1 due date').fill(local(Date.now() + 30 * DAY_MS, 10));
        await page.getByLabel('Action 1 severity').selectOption('critical');
        await page.getByLabel('Action 1 retest kind').selectOption('run');
        await page.getByLabel('Action 1 retest id').fill('exp-vpn-retest');
        await page.getByRole('button', { name: 'Save & submit' }).click();
        await expect(page.getByText(/submitted for verification/)).toBeVisible();
    });

    await test.step('the opened record shows what was recorded', async () => {
        await page.getByRole('tab', { name: 'Verification queue', exact: true }).click();
        await page.getByRole('cell', { name: NAME, exact: true }).click();
        await expect(page.getByTestId('signoff-summary')).toBeVisible();
        await expect(page.getByText('Tester: Red Team Ltd')).toBeVisible();
        await expect(
            page.getByText(/Attestation TLPT-ATT-2026-07 of \d{4}-\d{2}-\d{2}/),
        ).toBeVisible();
        await expect(page.getByText('Test summary report TSR-2026-07')).toBeVisible();
        const item = page.getByRole('listitem').filter({ hasText: 'Remove stale VPN accounts' });
        await expect(item).toContainText('critical');
        await expect(item).toContainText('retested by run');
        await expect(item).toContainText('exp-vpn-retest');
    });

    await test.step('another user signs it off, and its audit trail verifies', async () => {
        await actAs(page, 'bob');
        await page.getByRole('button', { name: 'Approve' }).click();
        await expect(page.getByText('Approved. Verified with 1 of 1 approvals.')).toBeVisible();
        await page.getByRole('button', { name: 'Verify audit trail' }).click();
        await expect(page.getByTestId('audit-verdict')).toContainText(
            'Audit trail ok: no problem found.',
        );
    });
});

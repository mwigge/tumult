// A DR exercise on the Manual page of a real daemon, end to end: a failover
// is planned, recorded against its plan with steps, people and an action,
// a PDF is attached, the record is submitted and signed off by two other
// users, its audit trail verifies, and the plan is fulfilled with the next
// one planned. Then a stored file is changed behind the API, and the page
// shows the verify and the download refuse it.
import { chmodSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test, type Page } from '@playwright/test';

import { startDaemon } from './daemon';
import type { Daemon } from './daemon';

// A failover needs two sign-offs in this house.
const ORG = `nodes:
  - {name: platform, kind: domain}
score:
  manual_required_signoffs: {failover: 2}
`;

const PDF = Buffer.from('%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\ntrailer << >>\n%%EOF\n');
const NAME = 'payments-db failover';
const DAY_MS = 86_400_000;

/** A local time as the value of a date or datetime-local field. */
function local(ms: number, length: 10 | 16 | 19): string {
    const date = new Date(ms);
    const shifted = new Date(ms - date.getTimezoneOffset() * 60_000);
    return shifted.toISOString().slice(0, length);
}

let daemon: Daemon;
const executed = Date.now() - 3_600_000;

test.describe.configure({ mode: 'serial' });

test.beforeAll(async () => {
    daemon = await startDaemon(ORG);
});

test.afterAll(() => {
    daemon?.stop();
});

async function actAs(page: Page, name: string) {
    await page.getByPlaceholder('your name').fill(name);
}

async function openRecord(page: Page, tab: 'Verification queue' | 'All records') {
    await page.getByRole('button', { name: tab, exact: true }).click();
    await page.getByRole('cell', { name: NAME, exact: true }).click();
    await expect(page.getByTestId('signoff-summary')).toBeVisible();
}

test('a failover is planned, recorded, signed off twice and fulfils its plan', async ({ page }) => {
    test.setTimeout(120_000);
    await page.goto(`${daemon.base}/manual?tab=plans`);
    await actAs(page, 'alice');

    await test.step('plan the failover', async () => {
        await page.getByLabel('Plan name').fill(NAME);
        await page.getByLabel('Exercise type').selectOption('failover');
        await page.getByLabel('Owner').fill('alice');
        await page.getByLabel('Target system').fill('payments-db');
        await page.getByLabel('Target environment').fill('production');
        await page.getByLabel('Due date').fill(local(Date.now() + 7 * DAY_MS, 10));
        await page.getByLabel('Planned RTO (s)').fill('900');
        await page.getByLabel('Planned RPO (s)').fill('60');
        await page.getByRole('button', { name: 'Save plan' }).click();
        await expect(page.getByText(/^plan \w+ saved$/)).toBeVisible();
        const row = page.locator('tr[data-plan]');
        await expect(row).toHaveCount(1);
        await expect(row).toContainText('planned');
        await expect(row).toContainText('every 365 days');
    });

    await test.step('record it against the plan, with what the API refuses first', async () => {
        await page.getByRole('button', { name: 'New entry' }).click();
        await page.getByLabel('Experiment name').fill(NAME);
        await page.getByLabel('Exercise type').selectOption('failover');
        await page.getByLabel('Executed at').fill(local(executed, 16));
        await page.getByLabel('Plan it fulfils').selectOption({ index: 1 });
        await expect(page.getByLabel('Target system')).toHaveValue('payments-db');
        await expect(page.getByLabel('Planned RTO (s)')).toHaveValue('900');
        await page.getByLabel('Hypothesis', { exact: true }).fill('The standby takes over');
        await page.getByLabel('Method', { exact: true }).fill('Stop the primary, promote');
        await page.getByLabel('Actual recovery time (s)').fill('600');
        await page.getByLabel('Actual RPO (s)').fill('20');
        await page.getByLabel('Data verified').selectOption('yes');
        await page.getByRole('button', { name: 'Add participant' }).click();
        await page.getByLabel('Participant 1 name').fill('dave');
        await page.getByLabel('Participant 1 role').fill('DBA');
        for (const [n, title] of [
            [1, 'Stop the primary'],
            [2, 'Promote the standby'],
        ] as const) {
            await page.getByRole('button', { name: 'Add step' }).click();
            await page.getByLabel(`Step ${n} title`).fill(title);
            await page.getByLabel(`Step ${n} result`).selectOption('done');
        }
        await page.getByLabel('Step 1 start').fill(local(executed, 19));
        await page.getByLabel('Step 1 end').fill(local(executed + 60_000, 19));
        await page.getByLabel('Step 2 start').fill(local(executed + 120_000, 19));
        // The end of step 2 before its start: the API refuses it and says why.
        await page.getByLabel('Step 2 end').fill(local(executed + 60_000, 19));
        await page.getByRole('button', { name: 'Add action' }).click();
        await page.getByLabel('Action 1 title').fill('Raise the connection pool');
        await page.getByLabel('Action 1 owner').fill('dave');
        await page.getByLabel('Action 1 due date').fill(local(Date.now() + 30 * DAY_MS, 10));
        await page.getByLabel('Attestation text').fill('I attest this is the drill as run.');
        await page.getByRole('button', { name: 'Save draft' }).click();
        await expect(page.getByText('step 2 ends before it starts')).toBeVisible();

        await page.getByLabel('Step 2 end').fill(local(executed + 600_000, 19));
        await page.getByRole('button', { name: 'Save draft' }).click();
        await expect(page.getByText(/^draft \w+ saved$/)).toBeVisible();
    });

    await test.step('attach a PDF to the draft, and submit it', async () => {
        await page.getByLabel('File to attach').setInputFiles({
            name: 'runbook-log.pdf',
            mimeType: 'application/pdf',
            buffer: PDF,
        });
        await page.getByRole('button', { name: 'Upload file' }).click();
        await expect(page.getByText(/^Attached runbook-log\.pdf: application\/pdf/)).toBeVisible();
        await expect(page.getByRole('row', { name: /runbook-log\.pdf/ })).toContainText(
            `${PDF.length} B`,
        );
        await page.getByRole('button', { name: 'Save & submit' }).click();
        await expect(page.getByText(/submitted for verification/)).toBeVisible();
    });

    await test.step('the enterer cannot sign it off, and the page says why', async () => {
        await openRecord(page, 'Verification queue');
        await expect(page.getByTestId('signoff-summary')).toHaveText(
            '0 of 2 approvals. 2 more sign-offs needed.',
        );
        await page.getByRole('button', { name: 'Approve' }).click();
        await expect(page.getByText(/reviewer must not have created/)).toBeVisible();
    });

    await test.step('two other users sign it off', async () => {
        await actAs(page, 'bob');
        await page.getByRole('button', { name: 'Approve' }).click();
        await expect(
            page.getByText('Approved. 1 of 2 approvals. 1 more sign-off needed.'),
        ).toBeVisible();
        await expect(page.getByTestId('signoff-summary')).toHaveText(
            '1 of 2 approvals. 1 more sign-off needed.',
        );
        await actAs(page, 'carol');
        await page.getByRole('button', { name: 'Approve' }).click();
        await expect(page.getByText('Approved. Verified with 2 of 2 approvals.')).toBeVisible();
    });

    await test.step('its audit trail verifies, and the file downloads as sent', async () => {
        await page.getByRole('button', { name: 'Verify audit trail' }).click();
        await expect(page.getByTestId('audit-verdict')).toContainText(
            'Audit trail ok: no problem found.',
        );
        const download = page.waitForEvent('download');
        await page.getByRole('button', { name: 'Download runbook-log.pdf' }).click();
        const file = await download;
        expect(file.suggestedFilename()).toBe('runbook-log.pdf');
        const path = await file.path();
        expect(path).toBeTruthy();
        const { readFileSync } = await import('node:fs');
        expect(readFileSync(path)).toEqual(PDF);
    });

    await test.step('the plan is fulfilled, and the next one is planned', async () => {
        await page.getByRole('button', { name: 'Plans' }).click();
        const rows = page.locator('tr[data-plan]');
        await expect(rows).toHaveCount(2);
        await expect(rows.filter({ hasText: 'fulfilled' })).toHaveCount(1);
        const next = rows.filter({ hasText: 'planned' });
        await expect(next).toHaveCount(1);
        await expect(next).toContainText(local(executed + 365 * DAY_MS, 10));
    });
});

test('a stored file changed behind the API fails the verify and is not served', async ({
    page,
}) => {
    const store = join(daemon.home, 'attachments');
    const [shard] = readdirSync(store).filter((d) => /^[0-9a-f]{2}$/.test(d));
    const [stored] = readdirSync(join(store, shard)).filter((f) => /^[0-9a-f]{64}$/.test(f));
    chmodSync(join(store, shard, stored), 0o644);
    writeFileSync(join(store, shard, stored), Buffer.from('%PDF-1.4\nchanged\n%%EOF\n'));

    await page.goto(`${daemon.base}/manual`);
    await actAs(page, 'erin');
    await openRecord(page, 'All records');
    await page.getByRole('button', { name: 'Verify audit trail' }).click();
    const verdict = page.getByTestId('audit-verdict');
    await expect(verdict).toContainText(/Audit trail not ok: \d+ problems?\./);
    await expect(
        verdict.getByRole('listitem').filter({ hasText: 'evidence_attachments' }),
    ).toContainText('file_hash');

    await page.getByRole('button', { name: 'Download runbook-log.pdf' }).click();
    await expect(
        page.getByText('the stored file does not match its hash: run the audit verify'),
    ).toBeVisible();
});

// The plan scenarios of RTS 2024/1774 against a real daemon: `tumult
// validate` refuses an id that is not one, `tumult run` hands the daemon a
// run that tests a cyber attack, the Manual page shows why the API refuses
// a mistyped scenario and keeps a known one, and the DORA view shows each
// scenario of the critical function as the score reads the lake.
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test, type Page } from '@playwright/test';

import { binary, startDaemon } from './daemon';
import type { Daemon } from './daemon';

// A firm under the simplified framework with one critical function.
const ORG = `nodes:
  - {name: platform, kind: domain}
legal_entities:
  - lei: 529900T8BM49AURSDO55
    name: Live Firm
    profile: simplified
functions:
  - id: cif-0001
    name: Payments
    tier: critical
    org_unit: platform
    services: [payments-api]
`;

// A run of payments-api that holds its steady state and names `scenarios`.
const experiment = (scenarios: string) => `title: Payments rides out a cyber attack
description: A probe reads a file that no step changes

target:
  system: payments-api
  environment: live

steady_state_hypothesis:
  title: The state is ok
  probes[1]:
    - name: state
      activity_type: probe
      provider:
        type: process
        path: sh
        arguments[2]: "-c", "cat state"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "^ok$"

method[1]:
  - name: look
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat state"
      timeout_s: 5.0

regulatory:
  frameworks[1]: DORA
  requirements[1]:
    - id: rts1774.art39
      description: The continuity plan tested against a cyber attack
      evidence: The journal of this run
  scenarios[1]: ${scenarios}
`;

/** A local time as the value of a datetime-local field. */
function local(ms: number): string {
    const date = new Date(ms);
    return new Date(ms - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}

let daemon: Daemon;

test.describe.configure({ mode: 'serial' });

test.beforeAll(async () => {
    daemon = await startDaemon(ORG);
    writeFileSync(join(daemon.home, 'state'), 'ok\n');
});

test.afterAll(() => {
    daemon?.stop();
});

function tumult(args: string[], file: string, scenarios: string) {
    writeFileSync(join(daemon.home, file), experiment(scenarios));
    return spawnSync(binary('TUMULT_BIN'), [...args, file], {
        cwd: daemon.home,
        env: daemon.env({ TUMULT_DAEMON_URL: daemon.base }),
        encoding: 'utf8',
    });
}

test('tumult validate refuses a scenario the RTS does not name and a run is tagged', () => {
    const refused = tumult(['validate'], 'typo.toon', 'cyber_attacks');
    expect(refused.status).not.toBe(0);
    expect(refused.stderr).toContain(
        "'cyber_attacks' is not a scenario id of RTS 2024/1774 Arts. 25, 26 and 39; " +
            "did you mean 'cyber_attack'?",
    );
    const valid = tumult(['validate'], 'tagged.toon', 'cyber_attack');
    expect(valid.status, valid.stderr).toBe(0);
    const run = tumult(['run'], 'tagged.toon', 'cyber_attack');
    expect(run.status, run.stderr).toBe(0);
});

async function fillTabletop(page: Page, scenarios: string) {
    await page.getByLabel('Experiment name').fill('payments restore tabletop');
    await page.getByLabel('Exercise type').selectOption('tabletop');
    await page.getByLabel('Executed at').fill(local(Date.now() - 3_600_000));
    await page.getByLabel('Target system').fill('payments-api');
    await page.getByLabel('Target environment').fill('live');
    await page.getByLabel('Hypothesis', { exact: true }).fill('The backup restores the ledger');
    await page.getByLabel('Method', { exact: true }).fill('Walk through the restore runbook');
    await page.getByLabel('Plan scenarios (comma-sep)').fill(scenarios);
    await page.getByLabel('Attestation text').fill('I attest this is the tabletop as run.');
}

test('the Manual page shows why a mistyped scenario is refused and keeps a known one', async ({
    page,
}) => {
    await page.goto(`${daemon.base}/manual`);
    await page.getByPlaceholder('your name').fill('alice');
    await page.getByRole('tab', { name: 'New entry' }).click();
    await fillTabletop(page, 'backup_restor');
    await page.getByRole('button', { name: 'Save draft' }).click();
    await expect(
        page.getByText(
            "'backup_restor' is not a scenario id of RTS 2024/1774 Arts. 25, 26 and 39; " +
                "did you mean 'backup_restore'?",
        ),
    ).toBeVisible();

    await page.getByLabel('Plan scenarios (comma-sep)').fill('backup_restore');
    await page.getByRole('button', { name: 'Save draft' }).click();
    await expect(page.getByText(/^draft \w+ saved$/)).toBeVisible();
    const answer = await fetch(`${daemon.base}/api/manual/experiments`);
    const { records } = (await answer.json()) as { records: { scenarios: string[] }[] };
    expect(records.map((r) => r.scenarios)).toEqual([['backup_restore']]);
});

test('the DORA view shows the scenarios of the critical function as the lake has them', async ({
    page,
}) => {
    await page.goto(`${daemon.base}/scores?view=dora`);
    const panel = page.getByTestId('dora-scenarios');
    await expect(panel).toContainText('rts1774.art39');
    const cyber = panel.locator('tr[data-scenario="cyber_attack"]');
    await expect(cyber.locator('.badge')).toHaveText('passed');
    // A draft is no evidence: the restore stays missing until it is verified.
    const restore = panel.locator('tr[data-scenario="backup_restore"]');
    await expect(restore.locator('.badge')).toHaveText('missing');
    await expect(panel.locator('tr[data-counts]')).toContainText('1 of 2');
});

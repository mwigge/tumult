// The Disaster recovery page of a real daemon: `tumultd` serves the page and
// `GET /api/dr` from a lake of its own. Two real DR runs of the ledger hand
// it their journals: one recovers, one does not. Manual DR exercises are
// entered and signed off through the API, and a backup is planned. What the
// page shows is compared with what the API and the journals say.
import { spawnSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import { seconds } from '../src/lib/dr';
import type { DrView } from '../src/lib/types';
import { binary, startDaemon } from './daemon';
import type { Daemon } from './daemon';

const DAY_MS = 86_400_000;

/**
 * A DR test of the ledger that needs only a shell. Its probe reads `down`
 * until the deadline the method writes, then `ok`: an outage of `outage_s`
 * seconds after the method. The post phase watches for `window_s` seconds.
 */
function drExperiment(title: string, file: string, outageS: number, windowS: number): string {
    return `title: ${title}
description: The ledger fails over, as a shell tells it
kind: dr
rto_target_s: 5
target:
  system: ledger
  environment: prod
recovery:
  window_s: ${windowS}
  interval_s: 0.5

steady_state_hypothesis:
  title: The ledger answers
  probes[1]:
    - name: ledger
      activity_type: probe
      provider:
        type: process
        path: sh
        arguments[2]: "-c", "if [ $(date +%s) -lt $(cat ${file} 2>/dev/null || echo 0) ]; then echo down; else echo ok; fi"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "^ok$"

method[1]:
  - name: fail over
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo $(( $(date +%s) + ${outageS} )) > ${file}"
      timeout_s: 5.0

rollbacks[0]:
`;
}

interface Verdict {
    nfr_id: string;
    observed: number | null;
    recovered?: boolean | null;
}

interface Journal {
    experiment_id: string;
    experiment_title: string;
    status: string;
    objectives?: { rto_measured_s: number | null; verdicts: Verdict[] } | null;
}

let daemon: Daemon;

/** A real run of `toon`, handed to the daemon; its journal as it printed it. */
function run(name: string, toon: string): Journal {
    writeFileSync(join(daemon.home, `${name}.toon`), toon);
    const done = spawnSync(
        binary('TUMULT_BIN'),
        [
            'run',
            `${name}.toon`,
            '--output-format',
            'json',
            '--journal-path',
            `${name}.journal.toon`,
        ],
        { cwd: daemon.home, env: daemon.env({ TUMULT_DAEMON_URL: daemon.base }), encoding: 'utf8' },
    );
    expect(done.stdout, `the run printed its journal:\n${done.stderr}`).toContain('experiment_id');
    return JSON.parse(done.stdout) as Journal;
}

async function call(method: string, path: string, body?: unknown): Promise<{ id?: string }> {
    const answer = await fetch(`${daemon.base}${path}`, {
        method,
        headers: { 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await answer.text();
    expect(answer.ok, `${method} ${path}: ${answer.status} ${text}`).toBe(true);
    return JSON.parse(text);
}

/** A manual DR exercise, entered by alice and signed off by bob. */
async function exercise(fields: Record<string, unknown>): Promise<string> {
    const { id } = await call('POST', '/api/manual/experiments', {
        hypothesis: 'The standby takes the writes within the RTO',
        method: 'Stop the primary and promote the standby',
        outcome_status: 'passed',
        hypothesis_met: true,
        target_environment: 'prod',
        entered_by: 'alice',
        attestation: 'I attest this record reflects the exercise as run.',
        ...fields,
    });
    if (!id) throw new Error('the record has an id');
    await call('POST', `/api/manual/experiments/${id}/submit`, { by: 'alice' });
    await call('POST', `/api/manual/experiments/${id}/verify`, { reviewer: 'bob' });
    return id;
}

/** A time `days` before now, in whole unix nanoseconds. */
const daysAgo = (days: number) => (Math.round(Date.now() / 1000) * 1000 - days * DAY_MS) * 1e6;

async function view(path = '/api/dr'): Promise<DrView> {
    const answer = await fetch(`${daemon.base}${path}`);
    expect(answer.status).toBe(200);
    return (await answer.json()) as DrView;
}

/** The text of each cell of the rows of the section `id`. */
async function rowsOf(page: Page, id: string): Promise<string[][]> {
    return page
        .locator(`section[aria-labelledby="${id}"] tbody tr`)
        .evaluateAll((rows) =>
            rows.map((row) =>
                [...row.querySelectorAll('th, td')].map((cell) =>
                    (cell.textContent ?? '').replace(/\s+/g, ' ').trim(),
                ),
            ),
        );
}

test.describe.configure({ mode: 'serial' });

test.beforeAll(async () => {
    daemon = await startDaemon();
});

test.afterAll(() => {
    daemon?.stop();
});

test('a lake with no DR test says so and what to do next', async ({ page }) => {
    await page.goto(`${daemon.base}/dr`);
    await expect(page.getByText('No DR tests yet.')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Manual page' })).toHaveAttribute(
        'href',
        '/manual',
    );
});

test.describe('with DR runs, exercises and a plan', () => {
    let recovered: Journal;
    let lost: Journal;
    let drill: string;

    test.beforeAll(async () => {
        // Two real runs, each watched for a few seconds.
        test.setTimeout(120_000);
        recovered = run('recovers', drExperiment('ledger failover', 'recovers.deadline', 2, 6));
        lost = run('never', drExperiment('ledger failover', 'never.deadline', 3600, 6));
        drill = await exercise({
            experiment_name: 'ledger failover drill',
            exercise_type: 'drill',
            executed_at_ns: daysAgo(60),
            target_system: 'ledger',
            recovery_time_s: 4,
            rto_target_s: 5,
            rpo_target_s: 30,
            rpo_measured_s: 12,
            data_verified: true,
        });
        await exercise({
            experiment_name: 'catalog restore',
            exercise_type: 'restore',
            executed_at_ns: daysAgo(350),
            target_system: 'catalog',
            recovery_time_s: 300,
            rto_target_s: 900,
        });
        await exercise({
            experiment_name: 'billing failover',
            exercise_type: 'failover',
            executed_at_ns: daysAgo(400),
            target_system: 'billing',
            recovery_time_s: 600,
            rto_target_s: 900,
        });
        await call('POST', '/api/manual/plans', {
            name: 'archive backup',
            exercise_type: 'backup',
            target_system: 'archive',
            target_environment: 'prod',
            owner: 'alice',
            due_at_ns: daysAgo(-20),
            rto_target_s: 120,
            rpo_target_s: 30,
            created_by: 'alice',
        });
    });

    /** The RTO verdict of a journal. */
    const rto = (journal: Journal) => {
        const verdict = journal.objectives?.verdicts.find((v) => v.nfr_id === 'rto');
        if (!verdict) throw new Error(`${journal.experiment_id} has an RTO verdict`);
        return verdict;
    };

    test('the runs are the DR tests they should be', () => {
        expect(rto(recovered).recovered).toBe(true);
        expect(recovered.objectives?.rto_measured_s).toBeGreaterThan(0);
        expect(rto(lost).recovered).toBe(false);
        expect(rto(lost).observed).toBeGreaterThan(3);
    });

    test('the register shows each target in the state the API says, and the least RTO of a run that did not recover', async ({
        page,
    }) => {
        const answer = await view();
        const ledger = answer.targets.find((t) => t.system === 'ledger');
        expect(ledger?.last_test?.id).toBe(`automated:${lost.experiment_id}`);
        expect(ledger?.rto_at_least_s).toBe(rto(lost).observed);

        await page.goto(`${daemon.base}/dr`);
        await expect(page.getByTestId('dr-register-summary')).toHaveText(
            '4 targets: 1 ok, 1 due, 1 overdue, 1 never tested',
        );
        const rows = await rowsOf(page, 'dr-register');
        expect(rows.map((r) => [r[0], r[1]])).toEqual([
            ['archive · prod', 'never tested'],
            ['billing · prod', 'overdue'],
            ['catalog · prod', 'due'],
            ['ledger · prod', 'ok'],
        ]);
        expect(answer.targets.map((t) => t.state)).toEqual([
            'never_tested',
            'overdue',
            'due',
            'ok',
        ]);
        const row = page.locator('tr[data-target="ledger/prod"]');
        await expect(row.locator('td').nth(5)).toHaveText(
            `at least ${seconds(rto(lost).observed ?? 0)}, not recovered target 5 s`,
        );
        await expect(row.locator('td').nth(1).getByRole('link')).toHaveAttribute(
            'href',
            `/experiments/${lost.experiment_id}`,
        );
        const archive = page.locator('tr[data-target="archive/prod"]');
        await expect(archive.getByRole('link', { name: /^due / })).toHaveAttribute(
            'href',
            '/manual?tab=plans',
        );
    });

    test('the RTO chart of the ledger draws what the runs and the drill measured', async ({
        page,
    }) => {
        await page.addInitScript(() => {
            (window as Window & { __tumultCharts?: object }).__tumultCharts = {};
        });
        await page.goto(`${daemon.base}/dr`);
        const host = page.locator('section[data-target="ledger/prod"] [data-chart^="dr-rto-"]');
        const id = (await host.getAttribute('data-chart')) ?? '';
        type Drawn = { series: { name: string; data: { value: number[] }[] }[] };
        const read = (chart: string) =>
            ((window as Window & { __tumultCharts?: Record<string, unknown> }).__tumultCharts?.[
                chart
            ] ?? { series: [] }) as Drawn;
        await expect.poll(async () => (await page.evaluate(read, id)).series.length).toBe(3);
        const drawn = await page.evaluate(read, id);
        const values = (n: number) => drawn.series[n].data.map((d) => d.value[1]);
        expect(drawn.series.map((s) => s.name)).toEqual([
            'measured',
            'manual',
            'not recovered, at least',
        ]);
        expect(values(0)).toEqual([4, recovered.objectives?.rto_measured_s]);
        expect(values(1)).toEqual([4]);
        expect(values(2)).toEqual([rto(lost).observed]);
    });

    test('the tests of the period come newest first and link to their evidence', async ({
        page,
    }) => {
        const answer = await view('/api/dr?limit=20');
        await page.goto(`${daemon.base}/dr`);
        const table = page.locator('section[aria-labelledby="dr-tests"]');
        await expect(table.getByRole('navigation')).toContainText('Page 1: tests 1 to 4 of 4');
        const ids = await table
            .locator('tbody tr')
            .evaluateAll((rows) => rows.map((r) => r.getAttribute('data-test')));
        expect(ids).toEqual(answer.tests.map((t) => t.test_id));
        expect(ids).toEqual([
            `automated:${lost.experiment_id}`,
            `automated:${recovered.experiment_id}`,
            `manual:${drill}`,
            expect.stringMatching(/^manual:/),
        ]);
        await expect(
            table.locator(`tr[data-test="manual:${drill}"]`).getByRole('link'),
        ).toHaveAttribute('href', `/manual?tab=records&record=${drill}`);
        await table
            .locator(`tr[data-test="automated:${lost.experiment_id}"]`)
            .getByRole('link')
            .click();
        await expect(page).toHaveURL(new RegExp(`/experiments/${lost.experiment_id}$`));
        await expect(page.locator('.page-head')).toContainText(lost.experiment_title);
    });

    test('a target picked in the filter is in the URL and in what the page shows', async ({
        page,
    }) => {
        await page.goto(`${daemon.base}/dr`);
        await page.getByLabel('Target').selectOption('ledger');
        await expect(page).toHaveURL(/\/dr\?target=ledger$/);
        await expect(page.locator('section[aria-labelledby="dr-register"] tbody tr')).toHaveCount(
            1,
        );
        await expect(page.locator('section[aria-labelledby="dr-tests"] tbody tr')).toHaveCount(3);
        await page.reload();
        await expect(page.getByLabel('Target')).toHaveValue('ledger');
        await expect(page.locator('section[aria-labelledby="dr-tests"] tbody tr')).toHaveCount(3);
    });
});

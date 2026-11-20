// The page of a run the daemon was asked for, read from a real daemon. The
// definition runs a command, so the daemon holds the run until someone
// approves it: the run has done nothing, and its page must not read as if
// it had.
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, test } from '@playwright/test';

import { startDaemon } from './daemon';
import type { Daemon } from './daemon';

const NOTE = 'one state file in the home of the daemon';

const EXPERIMENT = `title: A step breaks the state file
description: A probe reads a file, a step breaks it and a rollback heals it
blast_radius: ${NOTE}

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
  - name: break
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo down > state"
      timeout_s: 5.0

rollbacks[1]:
  - name: heal
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo ok > state"
      timeout_s: 5.0
`;

let daemon: Daemon;
let runId = '';

async function post(path: string, body: unknown): Promise<Record<string, unknown>> {
    const answer = await fetch(`${daemon.base}${path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
    });
    const text = await answer.text();
    expect(answer.status, text).toBeLessThan(300);
    return JSON.parse(text) as Record<string, unknown>;
}

test.beforeAll(async () => {
    daemon = await startDaemon();
    writeFileSync(join(daemon.home, 'state'), 'ok\n');
    const definition = await post('/api/runs/validate', { toon: EXPERIMENT });
    expect(definition).toMatchObject({ valid: true });
    const asked = await post('/api/runs', {
        registry_id: definition.registry_id,
        vars: {},
        env: 'dev',
    });
    expect(asked).toMatchObject({ state: 'pending_approval' });
    runId = String(asked.run_id);
});

test.afterAll(() => {
    daemon?.stop();
});

test('a run that waits for approval shows its blast radius as a plan', async ({ page }) => {
    await page.goto(`${daemon.base}/runs/${runId}`);

    const panel = page.getByTestId('run-scope');
    await expect(panel.getByRole('heading')).toHaveText('Planned blast radius');
    await expect(panel).toContainText(NOTE);
    await expect(panel.getByRole('listitem')).toContainText(['break']);
    await expect(panel.getByTestId('run-scope-plan')).toHaveText(
        'What the definition of this run plans to hit. It is a plan, not a measure of what the run did.',
    );

    // The run did nothing: the file its step breaks still says ok.
    await expect(page.locator('.badge').first()).toHaveText('pending_approval');
    expect(readFileSync(join(daemon.home, 'state'), 'utf8')).toBe('ok\n');
});

test('a run that waits for approval says how many approvals it has', async ({ page }) => {
    await page.goto(`${daemon.base}/runs/${runId}`);

    // A run of a command needs two approvals, and no one gave one yet.
    await expect(page.getByTestId('approval-quorum')).toHaveText('0/2');
});

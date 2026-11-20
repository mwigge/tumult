// The page of a run in Krönika, read from a real daemon: `tumultd` serves
// the page and its API from a lake of its own, `tumult run` runs a real
// experiment and hands the daemon its journal, and a recorded journal is
// imported as a user imports one. What the page shows is compared with
// what the journals say.
import { spawnSync } from 'node:child_process';
import { copyFileSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import type { Locator, Page } from '@playwright/test';

import { binary, startDaemon } from './daemon';
import type { Daemon } from './daemon';

const RECORDED = fileURLToPath(
    new URL('../../examples/recorded/api-saturation.journal.toon', import.meta.url),
);

// A probe reads a file, a step breaks it, a second probe sees it broken
// and a rollback heals it: a run of every phase that needs only a shell.
const EXPERIMENT = `title: The state file says ok until a step breaks it
description: A probe reads a file, a step breaks it and a rollback heals it

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

method[2]:
  - name: break
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo down > state"
      timeout_s: 5.0
  - name: look
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat state"
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

interface StepResult {
    name: string;
    activity_type: string;
    status: string;
    output?: string | null;
    error?: string | null;
}

interface Hypothesis {
    met: boolean;
    probe_results: StepResult[];
}

interface Verdict {
    nfr_id: string;
    metric: string;
    operator: string;
    threshold: number;
    observed: number | null;
    state: string;
    margin: number | null;
    criticality: string;
}

interface LoadWindow {
    ts_ns: number;
    throughput_rps: number;
    latency_p50_ms: number;
    latency_p95_ms: number;
    latency_p99_ms: number;
    error_rate: number;
}

interface Journal {
    experiment_id: string;
    experiment_title: string;
    status: string;
    kind?: string;
    started_at_ns: number;
    steady_state_before?: Hypothesis | null;
    steady_state_after?: Hypothesis | null;
    method_results: StepResult[];
    rollback_results: StepResult[];
    objectives?: { verdicts: Verdict[] } | null;
    load_result?: {
        tool: string;
        vus: number;
        total_requests: number;
        throughput_rps: number;
        latency_p50_ms: number;
        latency_p95_ms: number;
        latency_p99_ms: number;
        error_rate: number;
        thresholds_met: boolean;
        samples: LoadWindow[];
    } | null;
    probe_samples?: { probe: string; phase: string; ts_ns: number; ok: boolean }[];
}

let daemon: Daemon;
let real: Journal;
let recorded: Journal;

// A real run of the experiment. It hands its journal to the daemon, and
// prints it: that print is what the page is compared with.
function runExperiment(): Journal {
    writeFileSync(join(daemon.home, 'state'), 'ok\n');
    writeFileSync(join(daemon.home, 'experiment.toon'), EXPERIMENT);
    const run = spawnSync(
        binary('TUMULT_BIN'),
        ['run', 'experiment.toon', '--output-format', 'json'],
        { cwd: daemon.home, env: daemon.env({ TUMULT_DAEMON_URL: daemon.base }), encoding: 'utf8' },
    );
    expect(run.stdout, `the run printed its journal:\n${run.stderr}`).toContain('experiment_id');
    return JSON.parse(run.stdout) as Journal;
}

// The recorded journal as JSON, and in the lake of the daemon.
async function importRecorded(): Promise<Journal> {
    copyFileSync(RECORDED, join(daemon.home, 'recorded.toon'));
    const exported = spawnSync(
        binary('TUMULT_BIN'),
        ['export', 'recorded.toon', '--format', 'json'],
        {
            cwd: daemon.home,
            env: daemon.env({ TUMULT_LAKE_PATH: join(daemon.home, 'unused.duckdb') }),
            encoding: 'utf8',
        },
    );
    expect(exported.status, exported.stderr).toBe(0);
    const text = readFileSync(join(daemon.home, 'recorded.json'), 'utf8');
    // The text goes to the daemon as it is: a number of nanoseconds is
    // larger than a number of this language holds exactly.
    const answer = await fetch(`${daemon.base}/api/import/journal`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: `{"journal": ${text}}`,
    });
    expect(answer.status, await answer.clone().text()).toBe(200);
    expect(await answer.json()).toMatchObject({ ingested: true });
    return JSON.parse(text) as Journal;
}

test.beforeAll(async () => {
    // A run that ends out of its steady state waits for it to come back:
    // about half a minute.
    test.setTimeout(180_000);
    daemon = await startDaemon();
    real = runExperiment();
    recorded = await importRecorded();
});

test.afterAll(() => {
    daemon?.stop();
});

const PHASES: [string, (journal: Journal) => StepResult[]][] = [
    ['Steady state before', (journal) => journal.steady_state_before?.probe_results ?? []],
    ['Method', (journal) => journal.method_results],
    ['Steady state after', (journal) => journal.steady_state_after?.probe_results ?? []],
    ['Rollback', (journal) => journal.rollback_results],
];

const tidy = (text: string | null | undefined) => (text ?? '').replace(/\s+/g, ' ').trim();

// The steps of a journal as the page lists them: phase, name, type,
// status and what the step said.
function stepsOf(journal: Journal): string[][] {
    return PHASES.flatMap(([phase, results]) =>
        results(journal).map((step) => [
            phase,
            step.name,
            step.activity_type,
            step.status,
            tidy(step.error ?? step.output),
        ]),
    );
}

// The text of every cell of a table, row by row.
async function cells(table: Locator, rows: number): Promise<string[][]> {
    await expect(table.locator('tbody tr')).toHaveCount(rows);
    return table
        .locator('tbody tr')
        .evaluateAll((all) =>
            all.map((row) =>
                [...row.querySelectorAll('td')].map((cell) =>
                    (cell.textContent ?? '').replace(/\s+/g, ' ').trim(),
                ),
            ),
        );
}

// The steps table of the page without the two columns of time.
async function shownSteps(page: Page, rows: number): Promise<string[][]> {
    const table = page.getByTestId('evidence-steps').getByRole('table');
    return (await cells(table, rows)).map((row) => [...row.slice(0, 4), row[6]]);
}

interface Drawn {
    series: { name: string; data: unknown[]; markArea?: { data?: { name?: string }[][] } }[];
}

// What the chart `id` was drawn with.
async function drawn(page: Page, id: string): Promise<Drawn> {
    const read = (chart: string): Drawn => {
        const w = window as Window & { __tumultCharts?: Record<string, unknown> };
        const option = w.__tumultCharts?.[chart] as Drawn | undefined;
        const series = option?.series ?? [];
        return {
            series: series.map(({ name, data, markArea }) => ({ name, data, markArea })),
        };
    };
    await expect
        .poll(async () => (await page.evaluate(read, id)).series.length, {
            message: `chart ${id} is drawn`,
        })
        .toBeGreaterThan(0);
    return page.evaluate(read, id);
}

async function open(page: Page, journal: Journal) {
    await page.addInitScript(() => {
        (window as Window & { __tumultCharts?: object }).__tumultCharts = {};
    });
    await page.goto(`${daemon.base}/experiments/${journal.experiment_id}`);
    await expect(page.locator('.page-head')).toContainText(journal.experiment_title);
}

const met = (hypothesis: Hypothesis | null | undefined) => (hypothesis?.met ? 'met' : 'not met');

// A number as the page prints it: four digits that count.
const short = (value: number | null) =>
    value === null ? '—' : String(Number(value.toPrecision(4)));

const SIGNS: Record<string, string> = { lte: '≤', gte: '≥', eq: '=' };

test('a real run is in the list of runs and opens from it', async ({ page }) => {
    await page.goto(`${daemon.base}/experiments`);
    const row = page.locator('table.data tbody tr').filter({ hasText: real.experiment_title });
    await expect(row).toHaveCount(1);
    await expect(row).toContainText(real.status);
    await row.click();

    await expect(page).toHaveURL(new RegExp(`/experiments/${real.experiment_id}$`));
    await expect(page.locator('.page-head')).toContainText(real.experiment_title);
    await expect(page.locator('.page-head')).toContainText('journal only');
});

test('the page of a real run shows the steps and the steady state of its journal', async ({
    page,
}) => {
    // The run is the one the experiment tells: the journal is not empty.
    expect(stepsOf(real).map((step) => step.slice(0, 2))).toEqual([
        ['Steady state before', 'state'],
        ['Method', 'break'],
        ['Method', 'look'],
        ['Steady state after', 'state'],
        ['Rollback', 'heal'],
    ]);
    expect(real.steady_state_before?.met).toBe(true);
    expect(real.steady_state_after?.met).toBe(false);

    await open(page, real);

    expect(await shownSteps(page, 5)).toEqual(stepsOf(real));
    const steady = page.getByTestId('evidence-steady');
    await expect(steady.getByTestId('steady-before')).toHaveText('met');
    await expect(steady.getByTestId('steady-after')).toHaveText('not met');
    await expect(steady.getByTestId('run-kind')).toHaveText(real.kind ?? 'chaos');
    await expect(page.getByTestId('evidence-load')).toContainText('This run had no load.');
    await expect(page.getByTestId('no-evidence')).toHaveCount(0);
});

test('the page of a real run draws the probe samples of its journal', async ({ page }) => {
    const samples = real.probe_samples ?? [];
    expect(samples.length).toBeGreaterThan(0);
    await open(page, real);

    const panel = page.getByTestId('evidence-samples');
    await expect(panel.getByRole('heading')).toHaveText(`Probe samples (${samples.length})`);
    const chart = await drawn(page, 'run-probe-samples');
    const probes = [...new Set(samples.map((sample) => sample.probe))];
    expect(chart.series.map((series) => series.name)).toEqual(probes);
    for (const series of chart.series) {
        const own = samples.filter((sample) => sample.probe === series.name);
        expect(series.data).toHaveLength(own.length);
        // A sample out of its tolerance is the one point that is not a pair.
        expect(series.data.map((point) => Array.isArray(point))).toEqual(
            own.map((sample) => sample.ok),
        );
    }
});

test('the page of an imported run shows the steps, the objectives and the load of its journal', async ({
    page,
}) => {
    const load = recorded.load_result;
    const verdicts = recorded.objectives?.verdicts ?? [];
    if (!load) throw new Error('the recorded journal has a load');
    expect(verdicts.length).toBeGreaterThan(0);

    await open(page, recorded);

    expect(await shownSteps(page, stepsOf(recorded).length)).toEqual(stepsOf(recorded));
    const steady = page.getByTestId('evidence-steady');
    await expect(steady.getByTestId('steady-before')).toHaveText(met(recorded.steady_state_before));
    await expect(steady.getByTestId('steady-after')).toHaveText(met(recorded.steady_state_after));
    await expect(steady.getByTestId('run-kind')).toHaveText(recorded.kind ?? '');

    const objectives = page.getByTestId('evidence-objectives');
    await expect(objectives.getByRole('heading')).toHaveText(`Objectives (${verdicts.length})`);
    expect(await cells(objectives.getByRole('table'), verdicts.length)).toEqual(
        verdicts.map((verdict) => [
            verdict.nfr_id,
            verdict.metric,
            `${SIGNS[verdict.operator]} ${short(verdict.threshold)}`,
            short(verdict.observed),
            verdict.margin !== null && verdict.margin > 0
                ? `+${short(verdict.margin)}`
                : short(verdict.margin),
            verdict.state,
            verdict.criticality,
        ]),
    );

    const facts = await page
        .getByTestId('evidence-load')
        .locator('dl > div')
        .evaluateAll((all) =>
            all.map((fact) => [
                fact.querySelector('dt')?.textContent?.trim(),
                fact.querySelector('dd')?.textContent?.trim(),
            ]),
        );
    expect(facts).toEqual([
        ['Tool', load.tool],
        ['Virtual users', String(load.vus)],
        ['Requests', String(load.total_requests)],
        ['Throughput', `${load.throughput_rps.toFixed(1)} requests/s`],
        ['Latency p50', `${load.latency_p50_ms.toFixed(1)} ms`],
        ['Latency p95', `${load.latency_p95_ms.toFixed(1)} ms`],
        ['Latency p99', `${load.latency_p99_ms.toFixed(1)} ms`],
        ['Error rate', `${(load.error_rate * 100).toFixed(2)} %`],
        ['Thresholds', load.thresholds_met ? 'met' : 'not met'],
    ]);
});

test('the page of an imported run draws the windows of its load and its probe samples', async ({
    page,
}) => {
    const windows = recorded.load_result?.samples ?? [];
    const samples = recorded.probe_samples ?? [];
    expect(windows.length).toBeGreaterThan(1);
    expect(samples.some((sample) => sample.phase === 'during')).toBe(true);

    await open(page, recorded);

    // The second of each pair is the value as the journal has it.
    const values = (series: Drawn['series'][number]) =>
        series.data.map((point) => (point as number[])[1]);
    const latency = await drawn(page, 'run-load-latency');
    expect(latency.series.map((series) => [series.name, values(series)])).toEqual([
        ['p50', windows.map((window) => window.latency_p50_ms)],
        ['p95', windows.map((window) => window.latency_p95_ms)],
        ['p99', windows.map((window) => window.latency_p99_ms)],
    ]);
    const throughput = await drawn(page, 'run-load-throughput');
    expect(throughput.series[0].name).toBe('throughput');
    expect(values(throughput.series[0])).toEqual(windows.map((window) => window.throughput_rps));
    // A window is drawn where it began: after the start of the run, in order.
    const times = latency.series[0].data.map((point) => (point as number[])[0]);
    expect(times[0]).toBeGreaterThan(0);
    expect([...times].sort((a, b) => a - b)).toEqual(times);

    const panel = page.getByTestId('evidence-samples');
    await expect(panel.getByRole('heading')).toHaveText(`Probe samples (${samples.length})`);
    const chart = await drawn(page, 'run-probe-samples');
    expect(chart.series.map((series) => series.name)).toEqual([
        ...new Set(samples.map((sample) => sample.probe)),
    ]);
    expect(chart.series[0].data).toHaveLength(
        samples.filter((sample) => sample.probe === chart.series[0].name).length,
    );
    expect(chart.series[0].markArea?.data?.[0]?.[0]?.name).toBe('during the fault');
});

// The pages of Krönika with the API answers each one shows, for the style
// snapshot and the smoke tests, and the states of pages that a click opens. The answers of the chart, DORA, DR, run
// evidence and Star Map specs are reused; the other pages get small ones.
// Every time is fixed: the clock of the browser stands at NOW_MS.
import { expect } from '@playwright/test';
import type { Page } from '@playwright/test';

import type {
    AdminUser,
    ApiToken,
    ApprovalRequest,
    CatalogResponse,
    ExperimentRow,
    GameDayDetail,
    GameDayEntry,
    LogRow,
    ReportMetaV2,
    RunEvent,
    RunRow,
    Schedule,
    Span,
    Webhook,
} from '../src/lib/types';
import { bodies, HOUR_NS, NOW_MS, NOW_NS } from './chart-theme-data';
import { answerOf, fullLake } from './dr-data';
import { dora, maturity, tree } from './dora-data';
import { manualApi } from './manual-data';
import { experiment as evidenceRun, FULL } from './run-evidence-data';
import { demoStar } from './starmap-demo';

export { NOW_MS };

/** An answer that is not a 200. */
export class Refusal {
    constructor(
        readonly status: number,
        readonly body: unknown,
    ) {}
}

/** An answer that never comes: the page shows that it is loading. */
export const PENDING = Symbol('pending');

/** The answer to one call: a body, or a function of the URL and the method. */
type Answer = unknown | ((url: URL, method: string) => unknown);

export interface SnapshotPage {
    /** The name of the baseline files of the page. */
    name: string;
    path: string;
    /** The text of the h1 of the page once its data is shown; null for a page with none. */
    heading: string | null;
    /** The answers by path, beside `/api/me`. */
    api: Record<string, Answer>;
    /** For a state of a page: what it shows, and the clicks that open it. */
    shows?: string;
    open?: (page: Page) => Promise<void>;
}

const DAY_NS = 24 * HOUR_NS;
const SECOND_NS = 1_000_000_000;

const me = {
    auth_required: false,
    authenticated: true,
    username: null,
    role: null,
    env_scopes: [],
};

const experimentRow = (
    id: string,
    name: string,
    status: string,
    journalOnly: boolean,
): ExperimentRow => ({
    id,
    name,
    started_ns: NOW_NS - 2 * HOUR_NS,
    duration_ns: 30 * SECOND_NS,
    trace_id: journalOnly ? null : `trace-${id}`,
    target_system: 'payments-api',
    target_technology: 'postgresql',
    target_environment: 'staging',
    status,
    deviations: null,
    duration_ms: null,
    journal_only: journalOnly,
    faults: null,
    origin: 'automated',
    review_status: null,
});

const experiments = {
    count: 2,
    experiments: [
        experimentRow('exp-1', 'pg-failover', 'completed', false),
        experimentRow('cli-1', 'payments-replica-loss', 'deviated', true),
    ],
};

const run = (id: string, state: RunRow['state'], fields: Partial<RunRow> = {}): RunRow => ({
    id,
    registry_id: 'reg-1',
    state,
    gameday_id: null,
    params_json: { region: 'eu-north' },
    experiment_id: 'exp-1',
    rollback_status: null,
    error: null,
    queued_at_ns: NOW_NS - HOUR_NS,
    started_at_ns: NOW_NS - HOUR_NS + 90 * SECOND_NS,
    ended_at_ns: NOW_NS - HOUR_NS + 102.5 * SECOND_NS,
    definition_name: 'pg-failover',
    ...fields,
});

const runs = [
    run('run-1', 'passed'),
    run('run-2', 'running', { ended_at_ns: null }),
    run('run-3', 'failed', { error: 'the target did not answer' }),
];

const runDetail = (row: RunRow) => ({
    run: row,
    audit: [],
    approval: { request: null, decisions: [] },
});

// The plan of the definition with the vars of run-1.
const scope = {
    blast_radius: 'one region',
    actions: [
        {
            step: 'pause db',
            provider: 'docker',
            action: 'pause',
            targets: { container: 'pg-eu-north' },
        },
    ],
    guards: [],
    max_concurrent_faults: null,
};

const definition = {
    id: 'reg-demo',
    name: 'demo',
    content_hash: 'abcd',
    registered_at_ns: NOW_NS - DAY_NS,
    registered_by: 'operator',
    definition_toon: 'title: demo\nmethod[1]:\n  - name: ${host}\n',
};

const catalog: CatalogResponse = {
    action_count: 2,
    domains: [
        {
            domain: 'network',
            label: 'Network',
            actions: [
                {
                    plugin: 'tumult-network',
                    name: 'add-latency',
                    description: 'Add latency to the traffic of an interface',
                    kind: 'action',
                    args: [
                        { name: 'delay_ms', required: true, description: 'Latency to add' },
                        { name: 'jitter_ms', required: false, description: 'Jitter' },
                    ],
                },
                {
                    plugin: 'tumult-network',
                    name: 'ping',
                    description: 'Check that a host answers',
                    kind: 'probe',
                    args: [],
                },
            ],
        },
    ],
};

const gameday: GameDayDetail = {
    id: 'gd-1',
    title: 'smoke campaign',
    description: null,
    tags: [],
    regulatory: null,
    scoring: { pass_threshold: 0.75, mttr_target_s: 30, recovery_required: true },
    experiments: [
        { path: 'a.toon', compliance_maps: [], registry_id: 'reg-a', name: 'step A' },
        { path: 'b.toon', compliance_maps: [], registry_id: 'reg-b', name: 'step B' },
    ],
    registered_at_ns: NOW_NS - DAY_NS,
    registered_by: null,
};

const gamedays: GameDayEntry[] = [
    {
        id: 'gd-1',
        name: 'smoke campaign',
        content_hash: 'abcd',
        registered_at_ns: NOW_NS - DAY_NS,
        registered_by: 'operator',
    },
];

const campaign = run('camp-1', 'running', {
    registry_id: 'gd-1',
    params_json: { env: 'staging' },
    ended_at_ns: null,
});
const step = run('child-1', 'passed', { registry_id: 'reg-a', gameday_id: 'camp-1' });

const approval: ApprovalRequest = {
    run_id: 'run-9',
    state: 'awaiting_approval',
    queued_at_ns: NOW_NS - HOUR_NS,
    params_json: { region: 'eu-north' },
    definition_name: 'pg-failover',
    tier: 'T2',
    pin_hash: 'a'.repeat(64),
    env: 'production',
    target: 'cluster-a',
    quorum_required: 2,
    requested_by: 'operator',
    requested_at_ns: NOW_NS - HOUR_NS,
    expires_at_ns: NOW_NS + DAY_NS,
    consumed_at_ns: null,
    break_glass: false,
    break_glass_by: null,
    break_glass_justification: null,
    approved_count: 1,
};

const span = (id: string, parent: string | null, name: string, startMs: number, ms: number) =>
    ({
        ts_ns: NOW_NS - HOUR_NS + startMs * 1_000_000,
        trace_id: 't-1',
        span_id: id,
        parent_span_id: parent,
        span_name: name,
        span_kind: 'Internal',
        duration_ns: ms * 1_000_000,
        status_code: name === 'resilience.action' ? 'Error' : 'Unset',
        status_message: '',
        service_name: 'tumult',
        fault_type: null,
        fault_subtype: null,
        span_attrs: {},
        events: null,
    }) satisfies Span;

const spans = [
    span('root', null, 'resilience.experiment', 0, 5300),
    span('before', 'root', 'resilience.hypothesis.before', 0, 1),
    span('action', 'root', 'resilience.action', 2, 3000),
    span('after', 'root', 'resilience.hypothesis.after', 3010, 1),
];

const events: RunEvent[] = [
    {
        run_id: 'run-1',
        at_ns: NOW_NS - HOUR_NS,
        event: 'started',
        detail: null,
        actor: 'operator',
        prev_hash: null,
        new_hash: 'b'.repeat(64),
        definition_name: 'pg-failover',
    },
    {
        run_id: 'run-3',
        at_ns: NOW_NS - 2 * HOUR_NS,
        event: 'failed',
        detail: 'the target did not answer',
        actor: null,
        prev_hash: 'b'.repeat(64),
        new_hash: 'c'.repeat(64),
        definition_name: 'pg-failover',
    },
];

const users: AdminUser[] = [
    {
        id: 'u-1',
        username: 'admin',
        role: 'admin',
        must_change: false,
        disabled: false,
        created_at_ns: NOW_NS - 30 * DAY_NS,
        env_scopes: [],
    },
    {
        id: 'u-2',
        username: 'operator',
        role: 'operator',
        must_change: true,
        disabled: false,
        created_at_ns: NOW_NS - DAY_NS,
        env_scopes: ['staging'],
    },
];

const tokens: ApiToken[] = [
    {
        id: 'tok-1',
        user_id: 'u-1',
        username: 'admin',
        name: 'ci',
        created_at_ns: NOW_NS - 7 * DAY_NS,
        last_used_at_ns: NOW_NS - HOUR_NS,
        revoked: false,
        expires_at_ns: null,
    },
];

const webhooks: Webhook[] = [
    {
        id: 'wh-1',
        name: 'chat',
        url: 'https://hooks.example.com/tumult',
        events: ['run.failed'],
        enabled: true,
        created_by: 'admin',
        created_at_ns: NOW_NS - DAY_NS,
    },
];

const schedules: Schedule[] = [
    {
        id: 'sch-1',
        name: 'nightly failover',
        registry_id: 'reg-demo',
        definition_name: 'demo',
        interval_s: 86_400,
        vars_json: null,
        env: 'staging',
        target: null,
        enabled: true,
        next_run_at_ns: NOW_NS + 6 * HOUR_NS,
        last_run_at_ns: NOW_NS - 18 * HOUR_NS,
        last_run_id: 'run-1',
        created_by: 'operator',
        created_at_ns: NOW_NS - 7 * DAY_NS,
    },
];

const reports: ReportMetaV2[] = [
    {
        doc_id: 'doc-1',
        type: 'executive-digest',
        title: 'Executive digest',
        created_ns: NOW_NS - HOUR_NS,
        data_as_of_ns: NOW_NS - HOUR_NS,
        bytes: 20_480,
        sha256: 'd'.repeat(64),
        params: { period: '30d', experiment_id: null, framework: null },
    },
];

const conformance = {
    as_of_ns: NOW_NS,
    range: '24h',
    contract: {
        target: ['resilience.target.system'],
        spine: ['ict.schema.version', 'ict.entity.id'],
        recommended: ['service.version'],
    },
    summary: { services: 1, spine: 1, target: 0, none: 0, malformed_entity_ids: 0 },
    entity_id_pattern: '^[a-z][a-z0-9]*(-[a-z0-9]+)+$',
    malformed_entity_ids: [],
    services: [
        {
            service: 'checkout',
            spans: 120,
            last_seen_ns: NOW_NS - HOUR_NS,
            level: 'spine',
            missing: [],
            keys: [],
        },
    ],
};

const star = demoStar(NOW_NS);
const starmap = (url: URL) => ({
    as_of_ns: NOW_NS,
    range: url.searchParams.get('range') ?? '7d',
    env: null,
    focus: null,
    focus_found: null,
    depth: null,
    disclaimer: 'Evidence toward a requirement, not a statement of compliance.',
    truncated: false,
    maturity: null,
    counts: {
        nodes: star.nodes.length,
        edges: star.edges.length,
        pruned: 0,
        clustered: 0,
        risks_open: 0,
        risks_suppressed: 0,
        risks_unplaced: 0,
        gaps: 0,
    },
    clusters: [],
    ...star,
});

const lake = fullLake(NOW_MS);

const charts = (...paths: string[]) => Object.fromEntries(paths.map((p) => [p, bodies[p]]));

export const PAGES: SnapshotPage[] = [
    {
        name: 'overview',
        path: '/',
        heading: 'Overview',
        api: charts('/api/overview', '/api/experiments/windows'),
    },
    {
        name: 'resilience',
        path: '/resilience',
        heading: 'Resilience',
        api: {
            ...charts('/api/posture', '/api/risks', '/api/scores/tree'),
            '/api/otel/conformance': conformance,
        },
    },
    {
        name: 'performance',
        path: '/performance?target=checkout&env=demo',
        heading: 'Performance',
        api: charts('/api/perf', '/api/perf/checkout'),
    },
    { name: 'scores', path: '/scores', heading: 'Scores', api: charts('/api/scores/tree') },
    {
        name: 'dora',
        path: '/scores?view=dora',
        heading: 'Scores',
        api: {
            '/api/dora': (url: URL) => dora(url.searchParams.get('lei')),
            '/api/maturity': maturity,
            '/api/scores/tree': tree,
        },
    },
    {
        name: 'dr',
        path: '/dr',
        heading: 'Disaster recovery',
        api: { '/api/dr': (url: URL) => answerOf(lake, url, NOW_MS) },
    },
    {
        name: 'experiments',
        path: '/experiments',
        heading: 'Experiments',
        api: {
            '/api/experiments': experiments,
            '/api/dimensions': {
                outcomes: ['completed', 'deviated'],
                targets: ['payments-api'],
                faults: [],
                experiments: ['pg-failover', 'payments-replica-loss'],
            },
        },
    },
    {
        name: 'experiment',
        path: '/experiments/run-1',
        heading: 'checkout-saturation',
        api: {
            '/api/experiments/run-1': {
                experiment: evidenceRun,
                spans: [],
                logs: [],
                metrics: [],
                evidence: FULL,
            },
        },
    },
    { name: 'runs', path: '/runs', heading: 'Runs', api: { '/api/runs': { count: 3, runs } } },
    {
        name: 'run',
        path: '/runs/run-1',
        heading: 'pg-failover',
        api: {
            '/api/runs/run-1': runDetail(runs[0]),
            '/api/runs/run-1/impact': new Refusal(404, { error: 'not found' }),
            '/api/experiments/exp-1': { experiment: null, spans: [], logs: [], metrics: [] },
            '/api/runs/dry-run': { valid: true, registry_id: 'reg-1', plan: { scope } },
        },
    },
    {
        name: 'runs-new',
        path: '/runs/new?registry_id=reg-demo',
        heading: 'New run',
        api: {
            '/api/registry': { count: 1, definitions: [definition] },
            '/api/registry/reg-demo': { definition },
        },
    },
    {
        name: 'author',
        path: '/author',
        heading: 'Author an experiment',
        api: { '/api/authoring/catalog': catalog },
    },
    {
        name: 'author-new',
        path: '/author/new?plugin=tumult-network&action=add-latency',
        heading: 'Author an experiment',
        api: { '/api/authoring/catalog': catalog },
    },
    {
        name: 'gamedays',
        path: '/gamedays',
        heading: 'GameDays',
        api: { '/api/gamedays': { count: gamedays.length, gamedays } },
    },
    {
        name: 'gameday',
        path: '/gamedays/gd-1',
        heading: 'smoke campaign',
        api: {
            '/api/gamedays/gd-1': gameday,
            '/api/runs/camp-1': runDetail(campaign),
            '/api/runs': (url: URL) => {
                const found = url.searchParams.get('gameday_id') ? [step] : [campaign];
                return { count: found.length, runs: found };
            },
        },
    },
    {
        name: 'approvals',
        path: '/approvals',
        heading: 'Approvals',
        api: { '/api/approvals': { count: 1, queue: [approval] } },
    },
    {
        name: 'manual',
        path: '/manual',
        heading: 'Manual evidence',
        api: { '/api/manual/plans': { plans: [] }, '/api/manual/experiments': { records: [] } },
    },
    {
        name: 'logs',
        path: '/logs',
        heading: 'Logs',
        api: charts('/api/logs', '/api/logs/volume'),
    },
    {
        name: 'traces',
        path: '/traces',
        heading: 'Traces',
        api: charts('/api/traces', '/api/traces/durations'),
    },
    {
        name: 'trace',
        path: '/traces/t-1',
        heading: 't-1',
        api: { '/api/traces/t-1': { trace_id: 't-1', spans, logs: [] } },
    },
    {
        name: 'metrics',
        path: '/metrics?name=requests',
        heading: 'Metrics',
        api: charts('/api/metrics/catalog', '/api/metrics/query', '/api/experiments/windows'),
    },
    { name: 'starmap', path: '/starmap', heading: 'Star Map', api: { '/api/starmap': starmap } },
    { name: 'ask', path: '/ask', heading: 'Ask', api: {} },
    {
        name: 'reports',
        path: '/reports',
        heading: 'Reports',
        api: {
            '/api/reports/v2': { reports },
            '/api/reports': { reports: [] },
            '/api/experiments': experiments,
            '/api/metrics': { metrics: [] },
        },
    },
    {
        name: 'events',
        path: '/events',
        heading: 'Events',
        api: { '/api/events': { count: events.length, events } },
    },
    {
        name: 'users',
        path: '/users',
        heading: 'Users',
        api: { '/api/users': { users }, '/api/tokens': { tokens } },
    },
    {
        name: 'webhooks',
        path: '/webhooks',
        heading: 'Webhooks',
        api: { '/api/webhooks': { count: webhooks.length, webhooks } },
    },
    {
        name: 'schedules',
        path: '/schedules',
        heading: 'Schedules',
        api: {
            '/api/schedules': { count: schedules.length, schedules },
            '/api/registry': { count: 1, definitions: [definition] },
        },
    },
    { name: 'login', path: '/login', heading: null, api: {} },
];

const pageOf = (name: string) => PAGES.find((target) => target.name === name) as SnapshotPage;

/** Whether the page waits for an answer that never comes. */
export const holds = (target: SnapshotPage) => Object.values(target.api).includes(PENDING);

/** A state of the page `name`: its answers and `api`, at `path`, and its own heading if given. */
const stateOf = (
    name: string,
    state: Omit<SnapshotPage, 'heading' | 'api'> & {
        api?: Record<string, Answer>;
        heading?: string | null;
    },
): SnapshotPage => {
    const target = pageOf(name);
    return { ...target, ...state, api: { ...target.api, ...state.api } };
};

/** Click the first element of `selector`, then wait for `shown`. */
const opens = (selector: string, shown: string) => async (page: Page) => {
    await page.locator(selector).first().click();
    await expect(page.locator(shown).first()).toBeVisible();
};

/** Verify the audit trail of the open row, and wait for the verdict. */
const verify = async (page: Page) => {
    await page.getByRole('button', { name: 'Verify audit trail' }).click();
    await expect(page.locator('.verdict')).toBeVisible();
};

const logs: LogRow[] = [
    {
        ts_ns: NOW_NS - HOUR_NS,
        severity_text: 'ERROR',
        body: 'connection refused',
        trace_id: 't-1',
        span_id: 'action',
        log_attrs: { svc: 'api', 'experiment.id': 'run-1' },
    },
    {
        ts_ns: NOW_NS - 2 * HOUR_NS,
        severity_text: 'INFO',
        body: 'ready',
        trace_id: null,
        span_id: null,
        log_attrs: {},
    },
];

/** The answer of a read that failed. */
const failed = new Refusal(500, { error: 'lake query failed' });

/**
 * The states of pages that a click or the data opens: the Manual tabs with
 * data, open rows, the span drawer, the approval panel and filter chips,
 * and a page whose data failed or is on its way. Each has baselines of its
 * own.
 */
export const STATES: SnapshotPage[] = [
    stateOf('manual', {
        name: 'manual-plans-open',
        path: '/manual?tab=plans',
        shows: 'a plan open and its audit trail verified',
        api: manualApi,
        open: async (page) => {
            await page.locator('tr[data-plan="plan-1"]').click();
            await verify(page);
        },
    }),
    stateOf('manual', {
        name: 'manual-plans-cancel',
        path: '/manual?tab=plans',
        shows: 'the cancel form of a plan',
        api: manualApi,
        open: async (page) => {
            await page
                .locator('tr[data-plan="plan-2"]')
                .getByRole('button', { name: /cancel/i })
                .click();
            await expect(page.getByRole('button', { name: 'Keep it' })).toBeVisible();
        },
    }),
    stateOf('manual', {
        name: 'manual-queue-open',
        path: '/manual?tab=queue',
        shows: 'a record open and its audit trail verified',
        api: manualApi,
        open: async (page) => {
            await page.locator('tr[data-record="rec-1"]').click();
            await verify(page);
        },
    }),
    stateOf('manual', {
        name: 'manual-records',
        path: '/manual?tab=records',
        shows: 'three records',
        api: manualApi,
    }),
    stateOf('manual', {
        name: 'manual-import',
        path: '/manual?tab=import',
        shows: 'the import form',
        api: manualApi,
    }),
    stateOf('manual', {
        name: 'manual-entry-rows',
        path: '/manual',
        shows: 'a row in each list editor',
        api: manualApi,
        open: async (page) => {
            for (const name of ['Add participant', 'Add step', 'Add action'])
                await page.getByRole('button', { name }).click();
            await expect(page.locator('.grid-rows').first()).toBeVisible();
        },
    }),
    stateOf('run', {
        name: 'run-approval',
        path: '/runs/run-1',
        shows: 'an approval, an audit trail and a failed rollback',
        api: {
            '/api/runs/run-1': {
                run: { ...runs[0], state: 'pending_approval', rollback_status: 'failed' },
                audit: [
                    {
                        run_id: 'run-1',
                        at_ns: NOW_NS - HOUR_NS,
                        event: 'queued',
                        detail: null,
                        actor: 'operator',
                    },
                    {
                        run_id: 'run-1',
                        at_ns: NOW_NS - HOUR_NS / 2,
                        event: 'approval',
                        detail: 'one of two',
                        actor: 'bob',
                    },
                ],
                approval: {
                    request: { ...approval, run_id: 'run-1' },
                    decisions: [
                        {
                            run_id: 'run-1',
                            approver: 'bob',
                            decision: 'approved',
                            note: 'ok',
                            decided_at_ns: NOW_NS - HOUR_NS / 2,
                        },
                    ],
                },
            },
        },
    }),
    stateOf('trace', {
        name: 'trace-drawer',
        path: '/traces/t-1',
        shows: 'the span drawer open',
        api: {
            '/api/traces/t-1': {
                trace_id: 't-1',
                spans: spans.map((s) => ({ ...s, span_attrs: { 'db.system': 'postgresql' } })),
                logs: [],
            },
            ...charts('/api/experiments/windows'),
        },
        open: opens('button.row', '.drawer'),
    }),
    stateOf('logs', {
        name: 'logs-filtered',
        path: '/logs?attr=svc%3Dapi&attr_not=env%3Ddev',
        shows: 'two filter chips and an open row',
        api: { '/api/logs': { count: logs.length, logs } },
        open: opens('tr.clickable', 'tr.detail-row'),
    }),
    stateOf('traces', {
        name: 'traces-filtered',
        path: '/traces?attr=svc%3Dapi&attr_not=env%3Ddev',
        shows: 'two filter chips',
    }),
    stateOf('approvals', {
        name: 'approvals-open',
        path: '/approvals',
        shows: 'an open row',
        open: opens('tr.clickable', 'tr.detail-row'),
    }),
    stateOf('users', {
        name: 'users-open',
        path: '/users',
        shows: 'an open row',
        open: opens('tr.clickable', 'tr.detail-row'),
    }),
    stateOf('runs', {
        name: 'runs-error',
        path: '/runs',
        shows: 'a failed read',
        api: { '/api/runs': failed },
    }),
    stateOf('run', {
        name: 'run-error',
        path: '/runs/run-1',
        heading: 'Run',
        shows: 'a failed read',
        api: { '/api/runs/run-1': failed },
    }),
    stateOf('trace', {
        name: 'trace-loading',
        path: '/traces/t-1',
        heading: 'Trace',
        shows: 'its data on the way',
        api: { '/api/traces/t-1': PENDING },
    }),
];

/**
 * Answer every API call of `page` from its table. A call with no answer
 * is answered 404 and named in the list returned, so a test can fail on it.
 */
export async function serve(
    browser: import('@playwright/test').Page,
    page: SnapshotPage,
): Promise<string[]> {
    const missing: string[] = [];
    const table: Record<string, Answer> = { '/api/me': me, ...page.api };
    await browser.route('**/api/**', async (route) => {
        const url = new URL(route.request().url());
        const answer = table[url.pathname];
        if (answer === undefined) {
            missing.push(`${route.request().method()} ${url.pathname}`);
            await route.fulfill({ status: 404, json: { error: 'no mock' } });
            return;
        }
        const body = typeof answer === 'function' ? answer(url, route.request().method()) : answer;
        if (body === PENDING) return;
        if (body instanceof Refusal) await route.fulfill({ status: body.status, json: body.body });
        else await route.fulfill({ json: body });
    });
    return missing;
}

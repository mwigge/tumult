<script lang="ts">
    // Evidence drawer of the Star Map: what is known about the selected
    // node. The map response only carries counts; the rows are loaded here,
    // lazily, from the endpoints the other pages already use. Each section
    // loads and fails on its own. It is a Drawer in the page, not modal, so
    // the map and the list beside it still work. Opening the drawer moves
    // the focus to its heading; Escape closes it and the page hands the
    // focus back to the button of the node.
    import { api, fmtAgo, fmtDuration } from '#lib/api.js';
    import type {
        ExperimentRow,
        GateVerdict,
        ManualExperiment,
        NfrCell,
        Risk,
        StarNode,
    } from '#lib/types.js';
    import { disciplineLabel } from '#lib/coverage.js';
    import { experimentDurationNs } from '#lib/duration.js';
    import { COMPLIANCE_SCORE, COMPLIANCE_SCORE_LINE } from '#lib/score-names.js';
    import { flagText, rollupLines } from '#lib/starmap/style.js';
    import { targetQuery } from '#lib/manual.js';
    import Badge from './Badge.svelte';
    import Drawer from './Drawer.svelte';
    import State from './State.svelte';

    let {
        node,
        range,
        onclose,
        onfocus,
    }: {
        node: StarNode;
        range: string;
        onclose: () => void;
        onfocus: (id: string) => void;
    } = $props();

    interface Section<T> {
        rows: T | null;
        error: string | null;
    }
    const empty = <T,>(): Section<T> => ({ rows: null, error: null });

    let runs: Section<ExperimentRow[]> = $state(empty());
    let gate: Section<GateVerdict> = $state(empty());
    let nfrs: Section<NfrCell[]> = $state(empty());
    let manual: Section<ManualExperiment[]> = $state(empty());
    let risks: Section<Risk[]> = $state(empty());

    const title = $props.id();
    let heading: HTMLElement | undefined = $state();
    const nodeId = $derived(node.id);
    const rollup = $derived(rollupLines(node));

    $effect(() => {
        // Each newly selected node announces itself; a reload of the same one does not.
        void nodeId;
        heading?.focus();
    });

    const isSystem = $derived(node.kind === 'system');
    // The Manual page and the DR page read the target of their URL as their filter.
    const query = $derived(targetQuery(node.label, node.env));
    const manualHref = $derived(`/manual?tab=records&${query}`);
    const hasRuns = $derived(node.kind === 'system' || node.kind === 'def');
    const disciplines = $derived(Object.entries(node.evidence.by_discipline));

    const sameEnv = (env: string | null) => node.env === null || env === node.env;

    function load<T>(request: Promise<T>, live: () => boolean, done: (s: Section<T>) => void) {
        request
            .then((rows) => live() && done({ rows, error: null }))
            .catch((e: unknown) => live() && done({ rows: null, error: String(e) }));
    }

    $effect(() => {
        const target = node.label;
        const env = node.env ?? '';
        const kind = node.kind;
        let cancelled = false;
        const live = () => !cancelled;
        runs = empty();
        gate = empty();
        nfrs = empty();
        manual = empty();
        risks = empty();
        if (kind === 'def') {
            // `name` is matched whole, so `worker-kill` never lists `api-worker-kill`.
            load(
                api.experiments({ name: target, range }).then((r) => r.experiments),
                live,
                (s) => (runs = s),
            );
        }
        if (kind === 'system') {
            load(
                api
                    .experiments({ target, range })
                    .then((r) => r.experiments.filter((x) => sameEnv(x.target_environment))),
                live,
                (s) => (runs = s),
            );
            load(api.gate(target, env ? { env } : {}), live, (s) => (gate = s));
            load(
                api
                    .posture(env)
                    .then((p) =>
                        p.nfrs.filter((n) => n.target === target && sameEnv(n.environment)),
                    ),
                live,
                (s) => (nfrs = s),
            );
            load(
                api
                    .manualList('')
                    .then((m) =>
                        m.records.filter(
                            (r) => r.target_system === target && sameEnv(r.target_environment),
                        ),
                    ),
                live,
                (s) => (manual = s),
            );
            load(
                api
                    .risks(env)
                    .then((r) =>
                        [...r.risks, ...r.suppressed].filter(
                            (x) => x.target === target && sameEnv(x.environment),
                        ),
                    ),
                live,
                (s) => (risks = s),
            );
        }
        return () => {
            cancelled = true;
        };
    });

    const day = (ns: number) => new Date(ns / 1_000_000).toISOString().slice(0, 10);
</script>

<Drawer labelledby={title} {onclose} modal={false} focus="heading" data-testid="starmap-drawer">
    <header>
        <div>
            <h2 class="title" id={title} tabindex="-1" bind:this={heading}>{node.label}</h2>
            <div class="sub mono">{node.id}</div>
        </div>
        <button class="close" onclick={onclose} aria-label="Close">×</button>
    </header>

    <section>
        <dl>
            <dt>kind</dt>
            <dd>{node.kind === 'org' && node.org_kind ? node.org_kind : node.kind}</dd>
            {#if node.env}
                <dt>environment</dt>
                <dd>{node.env}</dd>
            {/if}
            {#if node.tier}
                <dt>tier</dt>
                <dd>{node.tier}</dd>
            {/if}
            {#if node.posture}
                <dt>resilience</dt>
                <dd>{node.posture.score.toFixed(1)} ({node.posture.band})</dd>
            {/if}
            {#if node.conformance}
                <dt>conformance</dt>
                <dd>{node.conformance}</dd>
            {/if}
            <dt>evidence</dt>
            <dd>
                {node.evidence.runs} runs · {node.evidence.failed} failed · {node.evidence.manual}
                manual
                {#if node.evidence.last_run_ns !== null}
                    · last {fmtAgo(node.evidence.last_run_ns)}
                {/if}
            </dd>
            <dt>risks</dt>
            <dd>{node.risks.open} open · {node.risks.suppressed} suppressed</dd>
            {#if node.gap}
                <dt>gap</dt>
                <dd>{node.gap === 'missing' ? 'no evidence' : 'stale evidence'}</dd>
            {/if}
            {#if node.members !== null}
                <dt>collapsed</dt>
                <dd>{node.members} nodes</dd>
            {/if}
        </dl>
        {#if node.conflict}
            <p class="note">This service reports more than one entity id.</p>
        {/if}
        {#each node.flags as flag (flag)}
            <p class="note flag">Check: {flagText(flag)}.</p>
        {/each}
        {#each rollup as line (line)}
            <p class="note">{line}</p>
        {/each}
        {#if node.declared}
            <p class="note">Known from a declaration only; not seen in telemetry.</p>
        {/if}
        {#if node.risks.suppressed > 0}
            <p class="note">
                suppressed by {node.risks.suppressed_by ?? 'unknown'}
                {node.risks.suppression_expires_ns === null
                    ? 'with no expiry'
                    : `until ${day(node.risks.suppression_expires_ns)}`}: {node.risks
                    .suppression_reason ?? ''}
            </p>
        {/if}
        <div class="actions">
            <button onclick={() => onfocus(node.id)}>Focus here</button>
            {#if node.kind === 'service'}
                <a href="/traces?service={encodeURIComponent(node.label)}&range={range}">
                    Traces
                </a>
            {/if}
            {#if isSystem}
                <a href="/resilience{node.env ? `?env=${encodeURIComponent(node.env)}` : ''}">
                    Resilience
                </a>
                <a href={manualHref}>Manual records</a>
                <a href="/dr?{query}">Disaster recovery</a>
            {/if}
        </div>
    </section>

    {#if disciplines.length > 0}
        <section>
            <h3>Coverage by discipline</h3>
            <div class="chips">
                {#each disciplines as [discipline, status] (discipline)}
                    <Badge
                        kind="cover"
                        value={status}
                        label="{disciplineLabel(discipline)}: {status}"
                    />
                {/each}
            </div>
            {#if node.risks.recommended.length > 0}
                <p class="note">
                    recommended next: {node.risks.recommended.map(disciplineLabel).join(', ')}
                </p>
            {/if}
        </section>
    {/if}

    {#if hasRuns}
        <section>
            <h3>Runs, last {range}</h3>
            {#if runs.error}
                <State kind="error">Failed to load runs: {runs.error}</State>
            {:else if runs.rows === null}
                <State kind="loading" height={60} />
            {:else if runs.rows.length === 0}
                <p class="note">No runs in this window.</p>
            {:else}
                <ul class="rows">
                    {#each runs.rows as r (r.id)}
                        <li>
                            <Badge kind="status" value={r.status} label={r.status ?? '–'} />
                            <a href="/experiments/{encodeURIComponent(r.id)}">{r.name ?? r.id}</a>
                            <span class="meta">
                                {fmtAgo(r.started_ns)} · {fmtDuration(experimentDurationNs(r))}
                                {#if r.origin === 'manual'}· manual{/if}
                                {#if r.journal_only}· journal only{/if}
                            </span>
                        </li>
                    {/each}
                </ul>
            {/if}
        </section>
    {/if}

    {#if isSystem}
        <section>
            <h3>Gate verdict</h3>
            {#if gate.error}
                <p class="note">No gate verdict: {gate.error}</p>
            {:else if gate.rows === null}
                <State kind="loading" height={30} />
            {:else}
                <p>
                    <Badge kind="verdict" value={gate.rows.verdict} />
                    {COMPLIANCE_SCORE}
                    {gate.rows.score.toFixed(1)} ({gate.rows.band})
                </p>
                <p class="note">{COMPLIANCE_SCORE_LINE}</p>
                {#each gate.rows.reasons as reason (reason)}
                    <p class="note">{reason}</p>
                {/each}
            {/if}
        </section>

        <section>
            <h3>Objective verdicts</h3>
            {#if nfrs.error}
                <State kind="error">Failed to load verdicts: {nfrs.error}</State>
            {:else if nfrs.rows === null}
                <State kind="loading" height={30} />
            {:else if nfrs.rows.length === 0}
                <p class="note">No objectives were evaluated for this system.</p>
            {:else}
                <ul class="rows">
                    {#each nfrs.rows as n (n.experiment + n.nfr_id)}
                        <li>
                            <Badge kind="verdict" value={n.state} />
                            <span>{n.nfr_id}</span>
                            <span class="meta">
                                threshold {n.threshold} · observed {n.observed ?? '–'} · margin
                                {n.margin ?? '–'} · {n.experiment}
                            </span>
                        </li>
                    {/each}
                </ul>
            {/if}
        </section>

        <section>
            <h3>Manual records</h3>
            {#if manual.error}
                <State kind="error">Failed to load manual records: {manual.error}</State>
            {:else if manual.rows === null}
                <State kind="loading" height={30} />
            {:else if manual.rows.length === 0}
                <p class="note">No manual records for this system.</p>
            {:else}
                <ul class="rows">
                    {#each manual.rows as m (m.id)}
                        <li>
                            <Badge kind="review" value={m.status} />
                            <a href={manualHref}>{m.experiment_name}</a>
                            <span class="meta">
                                {m.outcome_status} · {fmtAgo(m.executed_at_ns)}
                            </span>
                        </li>
                    {/each}
                </ul>
            {/if}
        </section>

        <section>
            <h3>Risks</h3>
            {#if risks.error}
                <State kind="error">Failed to load risks: {risks.error}</State>
            {:else if risks.rows === null}
                <State kind="loading" height={30} />
            {:else if risks.rows.length === 0}
                <p class="note">No findings for this system.</p>
            {:else}
                <ul class="rows">
                    {#each risks.rows as r (r.id)}
                        <li class:dim={r.suppression !== null}>
                            <Badge kind="finding" value={r.severity} />
                            <span>{r.title}</span>
                            <span class="meta">
                                {r.detail}
                                {#if r.suppression}
                                    · suppressed by {r.suppression.actor}
                                    {r.suppression.expires_at_ns === null
                                        ? 'with no expiry'
                                        : `until ${day(r.suppression.expires_at_ns)}`}: {r
                                        .suppression.reason}
                                {/if}
                            </span>
                        </li>
                    {/each}
                </ul>
            {/if}
        </section>
    {/if}
</Drawer>

<style>
    header {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        margin-bottom: var(--space-3);
    }
    .title {
        font-size: var(--font-size-xl);
        font-weight: var(--font-weight-strong);
        word-break: break-all;
    }
    .flag {
        color: var(--warn);
    }
    .sub {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
        word-break: break-all;
    }
    .mono {
        font-family: var(--mono);
    }
    section {
        margin-bottom: var(--space-4);
    }
    dl {
        display: grid;
        grid-template-columns: 96px 1fr;
        gap: var(--space-1) var(--space-3);
        margin: 0;
        font-size: var(--font-size-xs);
    }
    dd {
        margin: 0;
        word-break: break-word;
    }
    .note {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
        margin: var(--space-2) 0 0;
    }
    .actions {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2) var(--space-3);
        align-items: center;
        margin-top: var(--space-3);
        font-size: var(--font-size-xs);
    }
    .actions button {
        background: var(--bg-hover);
        border: var(--border-width-sm) solid var(--border-strong);
        color: var(--text);
        border-radius: var(--radius-md);
        padding: var(--space-1) var(--space-3);
        cursor: pointer;
        font-size: var(--font-size-xs);
    }
    .chips {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2);
    }
    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
        font-size: var(--font-size-xs);
    }
    .rows li {
        padding: var(--space-2) 0;
        border-top: var(--border-width-sm) solid var(--border);
    }
    /* A faded row, not the colour and size of the global .dim. */
    .rows li.dim {
        opacity: 0.55;
        color: inherit;
        font-size: inherit;
    }
    .meta {
        display: block;
        color: var(--text-dim);
        font-size: var(--font-size-xs);
        margin-top: var(--space-1);
    }
</style>

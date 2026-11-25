<script lang="ts">
    // Evidence drawer of the Star Map: what is known about the selected
    // node. The map response only carries counts; the rows are loaded here,
    // lazily, from the endpoints the other pages already use. Each section
    // loads and fails on its own. Opening the drawer moves the focus to its
    // heading; Escape closes it and the page hands the focus back.
    import { api, fmtAgo, fmtDuration, statusClass } from '#lib/api.js';
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

    let heading: HTMLElement | undefined = $state();
    const nodeId = $derived(node.id);
    const rollup = $derived(rollupLines(node));

    $effect(() => {
        // Each newly selected node announces itself; a reload of the same one does not.
        void nodeId;
        heading?.focus();
    });

    /** Escape in a field belongs to the field: it clears a search box. */
    const typing = (target: EventTarget | null) =>
        target instanceof Element &&
        target.closest('input, textarea, select, [contenteditable]') !== null;

    function onKey(event: KeyboardEvent) {
        if (event.key !== 'Escape' || event.defaultPrevented) return;
        if (typing(event.target)) return;
        event.preventDefault();
        onclose();
    }
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

    const reviewClass = (status: string) =>
        status === 'verified' ? 'ok' : status === 'rejected' ? 'fail' : 'neutral';
    const verdictClass = (v: string) => (v === 'pass' ? 'ok' : v === 'warn' ? 'warn' : 'fail');
    const coverClass = (s: string) => (s === 'covered' ? 'ok' : s === 'stale' ? 'warn' : 'neutral');
    const severityClass = (s: string) => (s === 'critical' || s === 'high' ? 'fail' : 'warn');
    const nfrClass = (s: string) => (s === 'met' ? 'ok' : s === 'breached' ? 'fail' : 'neutral');
    const day = (ns: number) => new Date(ns / 1_000_000).toISOString().slice(0, 10);
</script>

<svelte:window onkeydown={onKey} />

<aside class="drawer" data-testid="starmap-drawer" aria-label="evidence">
    <header>
        <div>
            <h2 class="title" tabindex="-1" bind:this={heading}>{node.label}</h2>
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
                    <span class="badge {coverClass(status)}"
                        >{disciplineLabel(discipline)}: {status}</span
                    >
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
                <div class="state error">Failed to load runs: {runs.error}</div>
            {:else if runs.rows === null}
                <div class="skeleton" style="height: 60px"></div>
            {:else if runs.rows.length === 0}
                <p class="note">No runs in this window.</p>
            {:else}
                <ul class="rows">
                    {#each runs.rows as r (r.id)}
                        <li>
                            <span class="badge {statusClass(r.status)}">{r.status ?? '–'}</span>
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
                <div class="skeleton" style="height: 30px"></div>
            {:else}
                <p>
                    <span class="badge {verdictClass(gate.rows.verdict)}">{gate.rows.verdict}</span>
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
                <div class="state error">Failed to load verdicts: {nfrs.error}</div>
            {:else if nfrs.rows === null}
                <div class="skeleton" style="height: 30px"></div>
            {:else if nfrs.rows.length === 0}
                <p class="note">No objectives were evaluated for this system.</p>
            {:else}
                <ul class="rows">
                    {#each nfrs.rows as n (n.experiment + n.nfr_id)}
                        <li>
                            <span class="badge {nfrClass(n.state)}">{n.state}</span>
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
                <div class="state error">Failed to load manual records: {manual.error}</div>
            {:else if manual.rows === null}
                <div class="skeleton" style="height: 30px"></div>
            {:else if manual.rows.length === 0}
                <p class="note">No manual records for this system.</p>
            {:else}
                <ul class="rows">
                    {#each manual.rows as m (m.id)}
                        <li>
                            <span class="badge {reviewClass(m.status)}">{m.status}</span>
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
                <div class="state error">Failed to load risks: {risks.error}</div>
            {:else if risks.rows === null}
                <div class="skeleton" style="height: 30px"></div>
            {:else if risks.rows.length === 0}
                <p class="note">No findings for this system.</p>
            {:else}
                <ul class="rows">
                    {#each risks.rows as r (r.id)}
                        <li class:dim={r.suppression !== null}>
                            <span class="badge {severityClass(r.severity)}">{r.severity}</span>
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
</aside>

<style>
    .drawer {
        flex: 0 0 360px;
        max-width: 100%;
        background: var(--bg-raised);
        border: var(--border-width-sm) solid var(--border-strong);
        border-radius: var(--radius-xl);
        padding: var(--space-3-5) var(--space-4);
        overflow-y: auto;
        max-height: 640px;
    }
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
    .title:focus-visible {
        outline: var(--border-width-md) solid var(--accent);
        outline-offset: 2px;
    }
    .flag {
        color: var(--warn);
    }
    .sub {
        color: var(--text-dim);
        font-size: var(--font-size-2xs);
        word-break: break-all;
    }
    .mono {
        font-family: var(--mono);
    }
    .close {
        background: transparent;
        border: none;
        color: var(--text-dim);
        font-size: var(--font-size-2xl);
        cursor: pointer;
        line-height: 1;
    }
    section {
        margin-bottom: var(--space-4);
    }
    h3 {
        font-size: var(--font-size-3xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-md);
        color: var(--text-dim);
        margin-bottom: var(--space-2);
    }
    dl {
        display: grid;
        grid-template-columns: 96px 1fr;
        gap: 5px var(--space-2-5);
        margin: 0;
        font-size: var(--font-size-sm);
    }
    dt {
        color: var(--text-dim);
    }
    dd {
        margin: 0;
        word-break: break-word;
    }
    .note {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
        margin: var(--space-1-5) 0 0;
    }
    .actions {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2) var(--space-3);
        align-items: center;
        margin-top: var(--space-2-5);
        font-size: var(--font-size-sm);
    }
    .actions button {
        background: var(--bg-hover);
        border: var(--border-width-sm) solid var(--border-strong);
        color: var(--text);
        border-radius: var(--radius-md);
        padding: 3px var(--space-2-5);
        cursor: pointer;
        font-size: var(--font-size-xs);
    }
    .chips {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-1-5);
    }
    .rows {
        list-style: none;
        margin: 0;
        padding: 0;
        font-size: var(--font-size-sm);
    }
    .rows li {
        padding: var(--space-1-5) 0;
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
        font-size: var(--font-size-2xs);
        margin-top: var(--space-0-5);
    }
</style>

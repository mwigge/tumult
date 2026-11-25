<script lang="ts">
    import { page } from '$app/state';
    import { onMount } from 'svelte';
    import { ACTIVE_RUN_STATES, api } from '#lib/api.js';
    import { planVars } from '#lib/run-params.js';
    import type { DryRunScope, ExperimentDetail, MeResponse, RunDetail, Span } from '#lib/types.js';
    import Waterfall from '#lib/components/Waterfall.svelte';
    import SpanDrawer from '#lib/components/SpanDrawer.svelte';
    import RunHeader from '#lib/components/RunHeader.svelte';
    import ApprovalActions from '#lib/components/ApprovalActions.svelte';
    import AuditTimeline from '#lib/components/AuditTimeline.svelte';
    import ScopeSummary from '#lib/components/ScopeSummary.svelte';
    import RunImpactPanel from '#lib/components/RunImpactPanel.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';

    const id = $derived(page.params.id ?? '');

    let detail = $state<RunDetail | null>(null);
    let error = $state<string | null>(null);
    let me = $state<MeResponse | null>(null);
    let telemetry = $state<ExperimentDetail | null>(null);
    let selected = $state<Span | null>(null);
    // The planned blast radius: the definition of the run with its vars,
    // fetched once via the dry-run endpoint (Viewer-level; executes nothing).
    // A registered definition never changes, so this is the plan the run
    // had. It is not what the run did: a run can end before its method.
    let scope = $state<DryRunScope | null>(null);
    let scopeFailed = $state(false);

    // Approvers and admins may decide; only admins may break glass. The server
    // enforces both regardless — these only hide what the role cannot use.
    const canDecide = $derived(
        me
            ? !me.auth_required ||
                  (me.authenticated && (me.role === 'approver' || me.role === 'admin'))
            : false,
    );
    const isAdmin = $derived(
        me ? !me.auth_required || (me.authenticated && me.role === 'admin') : false,
    );

    // Set by /runs/new when a gated start redirected here: shows the
    // "awaiting approval" notice while the run is still pending.
    const awaitingTier = $derived(page.url.searchParams.get('awaiting'));

    // Operators and up may stop runs; missing role counts as viewer. When the
    // daemon has no users (open local mode) anyone may.
    const canStop = $derived(
        me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false,
    );
    const active = $derived(detail !== null && ACTIVE_RUN_STATES.has(detail.run.state));

    // The actor of the triggering (enqueued) audit event, else the first
    // audit entry that carries an actor.
    const triggerActor = $derived.by(() => {
        if (!detail) return null;
        return (
            detail.audit.find((a) => a.event === 'enqueued')?.actor ??
            detail.audit.find((a) => a.actor)?.actor ??
            null
        );
    });

    onMount(() => {
        api.me()
            .then((m) => (me = m))
            .catch(() => (me = null));
    });

    // Poll the run (and, once linked, its experiment telemetry) every 2s while
    // the state is active. Telemetry lags the run record — the batch span
    // exporter's final flush lands after the state flips — so a terminal run
    // whose spans have not arrived yet keeps polling for a short grace window
    // before the loop stops. The cleanup clears the pending tick, so no
    // intervals leak.
    $effect(() => {
        const runId = id;
        let cancelled = false;
        let timer: ReturnType<typeof setTimeout> | null = null;
        let lateTicks = 0;
        detail = null;
        telemetry = null;
        error = null;
        scope = null;
        scopeFailed = false;

        async function poll() {
            try {
                const d = await api.run(runId);
                if (cancelled) return;
                detail = d;
                error = null;
                if (!scope && !scopeFailed) {
                    try {
                        const r = await api.dryRun(d.run.registry_id, planVars(d.run.params_json));
                        if (cancelled) return;
                        if (r.valid) scope = r.plan.scope;
                        else scopeFailed = true;
                    } catch {
                        if (!cancelled) scopeFailed = true;
                    }
                }
                if (d.run.experiment_id) {
                    try {
                        const t = await api.experiment(d.run.experiment_id);
                        if (!cancelled) telemetry = t;
                    } catch {
                        // Telemetry lags the run record; keep the previous snapshot.
                    }
                }
                if (cancelled) return;
                if (ACTIVE_RUN_STATES.has(d.run.state)) {
                    timer = setTimeout(poll, 2000);
                } else if (
                    d.run.experiment_id &&
                    (!telemetry || telemetry.spans.length === 0) &&
                    lateTicks < 10
                ) {
                    // Terminal but spans not ingested yet: bounded grace polling.
                    lateTicks += 1;
                    timer = setTimeout(poll, 2000);
                }
            } catch (e) {
                if (cancelled) return;
                if (!detail) {
                    error = String(e);
                } else if (ACTIVE_RUN_STATES.has(detail.run.state)) {
                    // Transient failure mid-run: keep the last snapshot, retry.
                    timer = setTimeout(poll, 2000);
                }
            }
        }

        void poll();
        return () => {
            cancelled = true;
            if (timer) clearTimeout(timer);
        };
    });
</script>

{#if !detail}
    <PageHeader title="Run" back={{ href: '/runs', label: 'runs' }} />
{/if}
{#if error && !detail}
    <State kind="error" panel>{error}</State>
{:else if !detail}
    <State kind="loading" height={120} style="margin-bottom: var(--space-4)" />
    <State kind="loading" height={320} />
{:else}
    {@const run = detail.run}
    <RunHeader {run} runId={id} {active} {canStop} {awaitingTier} {triggerActor} />

    {#if scope}
        <Panel
            title="Planned blast radius"
            style="margin-bottom: var(--space-4)"
            data-testid="run-scope"
        >
            <ScopeSummary {scope} />
            <div class="plan-note" data-testid="run-scope-plan">
                What the definition of this run plans to hit. It is a plan, not a measure of what
                the run did.
            </div>
        </Panel>
    {/if}

    <Panel title="Telemetry" style="margin-bottom: var(--space-4)">
        {#if !run.experiment_id}
            <State kind="empty">Telemetry appears once the run starts executing.</State>
        {:else if !telemetry}
            <State kind="loading" height={200} />
        {:else if telemetry.spans.length === 0}
            <State kind="empty">No spans recorded yet.</State>
        {:else}
            <Waterfall spans={telemetry.spans} onselect={(s) => (selected = s)} />
            <div class="exp-link">
                <a href="/experiments/{run.experiment_id}">open the full experiment view →</a>
            </div>
        {/if}
    </Panel>

    {#if run.experiment_id && !active}
        <RunImpactPanel runId={id} />
    {/if}

    <ApprovalActions
        runId={id}
        runState={run.state}
        approval={detail.approval}
        {canDecide}
        {isAdmin}
    />

    <AuditTimeline audit={detail.audit} />

    {#if selected && telemetry}
        <SpanDrawer span={selected} logs={telemetry.logs} onclose={() => (selected = null)} />
    {/if}
{/if}

<style>
    .exp-link {
        margin-top: var(--space-3);
        font-size: var(--font-size-xs);
    }
    .plan-note {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
        margin-top: var(--space-2);
    }
</style>

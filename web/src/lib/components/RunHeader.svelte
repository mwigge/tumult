<script lang="ts">
    // Run detail header: title/status/stop action, run-level error and the
    // two-step e-stop confirm, the post-redirect "awaiting approval" notice,
    // and the run metadata grid (timestamps, duration, actor, rollback,
    // experiment/registry links).
    import { api, fmtAgo, fmtDuration, fmtTs } from '#lib/api.js';
    import { runTimeNs, runWaitNs } from '#lib/duration.js';
    import type { RunRow } from '#lib/types.js';
    import Badge from './Badge.svelte';
    import StatusBadge from './StatusBadge.svelte';
    import PageHeader from './PageHeader.svelte';
    import Panel from './Panel.svelte';

    let {
        run,
        runId,
        active,
        canStop,
        awaitingTier,
        triggerActor,
    }: {
        run: RunRow;
        runId: string;
        active: boolean;
        canStop: boolean;
        awaitingTier: string | null;
        triggerActor: string | null;
    } = $props();

    let confirmingStop = $state(false);
    let stopping = $state(false);
    let stopError = $state<string | null>(null);

    async function stop() {
        if (stopping) return;
        stopping = true;
        stopError = null;
        try {
            await api.stopRun(runId);
            confirmingStop = false;
            // Polling continues on its own cadence until the run goes terminal.
        } catch (e) {
            stopError = String(e);
        } finally {
            stopping = false;
        }
    }
</script>

<PageHeader title={run.definition_name ?? run.registry_id} back={{ href: '/runs', label: 'runs' }}>
    <StatusBadge status={run.state} />
    <span class="sub mono">{run.id}</span>
    {#if active && canStop && !confirmingStop}
        <button class="estop" onclick={() => (confirmingStop = true)}>Stop run</button>
    {/if}
</PageHeader>

{#if run.error}
    <div class="state error panel" style="margin-bottom: var(--space-3-5)">{run.error}</div>
{/if}

{#if confirmingStop && active}
    <div class="panel confirm" style="margin-bottom: var(--space-3-5)">
        <span>
            Confirm stop? This ends a running process or script step, starts no further step and
            runs the rollbacks.
        </span>
        <button class="estop" onclick={stop} disabled={stopping}>
            {stopping ? 'Stopping…' : 'Confirm stop'}
        </button>
        <button class="cancel" onclick={() => (confirmingStop = false)} disabled={stopping}
            >Cancel</button
        >
        {#if stopError}
            <span class="stop-error">{stopError}</span>
        {/if}
    </div>
{:else if stopError}
    <div class="state error panel" style="margin-bottom: var(--space-3-5)">{stopError}</div>
{/if}

{#if awaitingTier && run.state === 'pending_approval'}
    <div class="panel notice" style="margin-bottom: var(--space-3-5)">
        <span>
            Awaiting approval (tier {awaitingTier}) — the run starts once the quorum is met.
        </span>
        <a href="/approvals">view the approvals queue →</a>
    </div>
{/if}

<div class="grid" style="grid-template-columns: repeat(4, 1fr); margin-bottom: var(--space-3-5)">
    <Panel variant="meta" title="Queued">
        <b>{fmtTs(run.queued_at_ns)} ({fmtAgo(run.queued_at_ns)})</b>
    </Panel>
    <Panel variant="meta" title="Started">
        <b
            >{run.started_at_ns !== null
                ? `${fmtTs(run.started_at_ns)} (${fmtAgo(run.started_at_ns)})`
                : '—'}</b
        >
    </Panel>
    <Panel variant="meta" title="Ended">
        <b
            >{run.ended_at_ns !== null
                ? `${fmtTs(run.ended_at_ns)} (${fmtAgo(run.ended_at_ns)})`
                : '—'}</b
        >
    </Panel>
    <Panel variant="meta" title="Waited">
        <b class="mono">{fmtDuration(runWaitNs(run, Date.now() * 1e6))}</b>
    </Panel>
    <Panel variant="meta" title="Run time">
        <b class="mono">{fmtDuration(runTimeNs(run, Date.now() * 1e6))}</b>
    </Panel>
    <Panel variant="meta" title="Run by"><b>{triggerActor ?? 'system'}</b></Panel>
    <Panel variant="meta" title="Rollback">
        {#if run.state === 'rollback_pending'}
            <b><Badge tone="warn" label="rollback pending" /></b>
        {:else if run.rollback_status && run.rollback_status !== 'not_needed'}
            <b>
                <Badge
                    kind="rollback"
                    value={run.rollback_status}
                    label="rollback {run.rollback_status}"
                />
            </b>
        {:else}
            <b>—</b>
        {/if}
    </Panel>
    <Panel variant="meta" title="Experiment">
        <b class="mono">
            {#if run.experiment_id}
                <a href="/experiments/{run.experiment_id}">{run.experiment_id}</a>
            {:else}
                —
            {/if}
        </b>
    </Panel>
    <Panel variant="meta" title="Registry"><b class="mono">{run.registry_id}</b></Panel>
</div>

<style>
    button.estop {
        background: color-mix(in srgb, var(--fail) 16%, transparent);
        border: var(--border-width-sm) solid var(--fail);
        color: var(--fail);
        border-radius: var(--radius-lg);
        padding: var(--space-1-5) var(--space-3-5);
        font-size: var(--font-size-md);
        font-weight: var(--font-weight-strong);
        cursor: pointer;
    }
    button.estop:hover {
        background: color-mix(in srgb, var(--fail) 26%, transparent);
    }
    button.estop:disabled {
        opacity: 0.5;
        cursor: default;
    }
    .confirm {
        display: flex;
        align-items: center;
        gap: var(--space-3);
        flex-wrap: wrap;
        border-color: color-mix(in srgb, var(--fail) 55%, var(--border));
    }
    .confirm .cancel {
        background: transparent;
        border: var(--border-width-sm) solid var(--border-strong);
        color: var(--text-dim);
        border-radius: var(--radius-lg);
        padding: var(--space-1-5) var(--space-3-5);
        font-size: var(--font-size-md);
        cursor: pointer;
    }
    .confirm .cancel:hover {
        color: var(--text);
        border-color: var(--accent);
    }
    .stop-error {
        color: var(--fail);
        font-size: var(--font-size-sm);
    }
    .notice {
        display: flex;
        align-items: center;
        gap: var(--space-3);
        flex-wrap: wrap;
        border-color: color-mix(in srgb, var(--warn) 55%, var(--border));
        font-size: var(--font-size-md);
    }
</style>

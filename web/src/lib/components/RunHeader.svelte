<script lang="ts">
    // Run detail header: title/status/stop action, run-level error and the
    // two-step e-stop confirm, the post-redirect "awaiting approval" notice,
    // and the run metadata grid (timestamps, duration, actor, rollback,
    // experiment/registry links).
    import { api, fmtAgo, fmtDuration, fmtTs } from '#lib/api.js';
    import { runTimeNs, runWaitNs } from '#lib/duration.js';
    import type { RunRow } from '#lib/types.js';
    import StatusBadge from './StatusBadge.svelte';

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

<div class="page-head">
    <a href="/runs" style="color: var(--text-dim)">← runs</a>
    <h1>{run.definition_name ?? run.registry_id}</h1>
    <StatusBadge status={run.state} />
    <span class="sub mono">{run.id}</span>
    {#if active && canStop && !confirmingStop}
        <button class="estop" onclick={() => (confirmingStop = true)}>Stop run</button>
    {/if}
</div>

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
    <div class="panel meta">
        <span>Queued</span><b>{fmtTs(run.queued_at_ns)} ({fmtAgo(run.queued_at_ns)})</b>
    </div>
    <div class="panel meta">
        <span>Started</span>
        <b
            >{run.started_at_ns !== null
                ? `${fmtTs(run.started_at_ns)} (${fmtAgo(run.started_at_ns)})`
                : '—'}</b
        >
    </div>
    <div class="panel meta">
        <span>Ended</span>
        <b
            >{run.ended_at_ns !== null
                ? `${fmtTs(run.ended_at_ns)} (${fmtAgo(run.ended_at_ns)})`
                : '—'}</b
        >
    </div>
    <div class="panel meta">
        <span>Waited</span>
        <b class="mono">{fmtDuration(runWaitNs(run, Date.now() * 1e6))}</b>
    </div>
    <div class="panel meta">
        <span>Run time</span>
        <b class="mono">{fmtDuration(runTimeNs(run, Date.now() * 1e6))}</b>
    </div>
    <div class="panel meta"><span>Run by</span><b>{triggerActor ?? 'system'}</b></div>
    <div class="panel meta">
        <span>Rollback</span>
        {#if run.state === 'rollback_pending'}
            <b><span class="badge warn">rollback pending</span></b>
        {:else if run.rollback_status && run.rollback_status !== 'not_needed'}
            <b>
                <span class="badge {run.rollback_status === 'failed' ? 'fail' : 'ok'}">
                    rollback {run.rollback_status}
                </span>
            </b>
        {:else}
            <b>—</b>
        {/if}
    </div>
    <div class="panel meta">
        <span>Experiment</span>
        <b class="mono">
            {#if run.experiment_id}
                <a href="/experiments/{run.experiment_id}">{run.experiment_id}</a>
            {:else}
                —
            {/if}
        </b>
    </div>
    <div class="panel meta"><span>Registry</span><b class="mono">{run.registry_id}</b></div>
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

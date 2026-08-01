<script lang="ts">
  // Run detail header: title/status/stop action, run-level error and the
  // two-step e-stop confirm, the post-redirect "awaiting approval" notice,
  // and the run metadata grid (timestamps, duration, actor, rollback,
  // experiment/registry links).
  import { api, fmtAgo, fmtDuration, fmtTs } from '$lib/api';
  import type { RunRow } from '$lib/types';
  import StatusBadge from './StatusBadge.svelte';

  let {
    run,
    runId,
    active,
    canStop,
    awaitingTier,
    triggerActor
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
  <div class="state error panel" style="margin-bottom: 14px">{run.error}</div>
{/if}

{#if confirmingStop && active}
  <div class="panel confirm" style="margin-bottom: 14px">
    <span>
      Confirm stop? This halts the run before the next activity and unwinds rollbacks.
    </span>
    <button class="estop" onclick={stop} disabled={stopping}>
      {stopping ? 'Stopping…' : 'Confirm stop'}
    </button>
    <button class="cancel" onclick={() => (confirmingStop = false)} disabled={stopping}>Cancel</button>
    {#if stopError}
      <span class="stop-error">{stopError}</span>
    {/if}
  </div>
{:else if stopError}
  <div class="state error panel" style="margin-bottom: 14px">{stopError}</div>
{/if}

{#if awaitingTier && run.state === 'pending_approval'}
  <div class="panel notice" style="margin-bottom: 14px">
    <span>
      Awaiting approval (tier {awaitingTier}) — the run starts once the quorum is met.
    </span>
    <a href="/approvals">view the approvals queue →</a>
  </div>
{/if}

<div class="grid" style="grid-template-columns: repeat(4, 1fr); margin-bottom: 14px">
  <div class="panel meta"><span>Queued</span><b>{fmtTs(run.queued_at_ns)} ({fmtAgo(run.queued_at_ns)})</b></div>
  <div class="panel meta">
    <span>Started</span>
    <b>{run.started_at_ns !== null ? `${fmtTs(run.started_at_ns)} (${fmtAgo(run.started_at_ns)})` : '—'}</b>
  </div>
  <div class="panel meta">
    <span>Ended</span>
    <b>{run.ended_at_ns !== null ? `${fmtTs(run.ended_at_ns)} (${fmtAgo(run.ended_at_ns)})` : '—'}</b>
  </div>
  <div class="panel meta">
    <span>Duration</span>
    <b class="mono">
      {run.started_at_ns !== null
        ? fmtDuration((run.ended_at_ns ?? Date.now() * 1e6) - run.started_at_ns)
        : '—'}
    </b>
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
  .meta span {
    display: block;
    color: var(--text-dim);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    margin-bottom: 4px;
  }
  .meta b {
    font-weight: 600;
    font-size: 13.5px;
  }
  button.estop {
    background: color-mix(in srgb, var(--fail) 16%, transparent);
    border: 1px solid var(--fail);
    color: var(--fail);
    border-radius: 5px;
    padding: 6px 14px;
    font-size: 13px;
    font-weight: 600;
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
    gap: 12px;
    flex-wrap: wrap;
    border-color: color-mix(in srgb, var(--fail) 55%, var(--border));
  }
  .confirm .cancel {
    background: transparent;
    border: 1px solid var(--border-strong);
    color: var(--text-dim);
    border-radius: 5px;
    padding: 6px 14px;
    font-size: 13px;
    cursor: pointer;
  }
  .confirm .cancel:hover {
    color: var(--text);
    border-color: var(--accent);
  }
  .stop-error {
    color: var(--fail);
    font-size: 12.5px;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    border-color: color-mix(in srgb, var(--warn) 55%, var(--border));
    font-size: 13px;
  }
</style>

<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { ACTIVE_RUN_STATES, api, fmtAgo, fmtDuration, fmtTs } from '$lib/api';
  import type { ExperimentDetail, MeResponse, RunDetail, Span } from '$lib/types';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import Waterfall from '$lib/components/Waterfall.svelte';
  import SpanDrawer from '$lib/components/SpanDrawer.svelte';

  const id = $derived($page.params.id ?? '');

  let detail = $state<RunDetail | null>(null);
  let error = $state<string | null>(null);
  let me = $state<MeResponse | null>(null);
  let telemetry = $state<ExperimentDetail | null>(null);
  let selected = $state<Span | null>(null);

  let confirmingStop = $state(false);
  let stopping = $state(false);
  let stopError = $state<string | null>(null);

  // Operators and up may stop runs; missing role counts as viewer. When the
  // daemon has no users (open local mode) anyone may.
  const canStop = $derived(
    me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false
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
    api
      .me()
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

    async function poll() {
      try {
        const d = await api.run(runId);
        if (cancelled) return;
        detail = d;
        error = null;
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

  async function stop() {
    if (stopping) return;
    stopping = true;
    stopError = null;
    try {
      await api.stopRun(id);
      confirmingStop = false;
      // Polling continues on its own cadence until the run goes terminal.
    } catch (e) {
      stopError = String(e);
    } finally {
      stopping = false;
    }
  }
</script>

{#if error && !detail}
  <div class="page-head">
    <a href="/runs" style="color: var(--text-dim)">← runs</a>
    <h1>Run</h1>
  </div>
  <div class="state error panel">{error}</div>
{:else if !detail}
  <div class="page-head">
    <a href="/runs" style="color: var(--text-dim)">← runs</a>
    <h1>Run</h1>
  </div>
  <div class="skeleton" style="height: 120px; margin-bottom: 14px"></div>
  <div class="skeleton" style="height: 320px"></div>
{:else}
  {@const run = detail.run}
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

  <div class="panel" style="margin-bottom: 14px">
    <h2>Telemetry</h2>
    {#if !run.experiment_id}
      <div class="state">Telemetry appears once the run starts executing.</div>
    {:else if !telemetry}
      <div class="skeleton" style="height: 200px"></div>
    {:else if telemetry.spans.length === 0}
      <div class="state">No spans recorded yet.</div>
    {:else}
      <Waterfall spans={telemetry.spans} onselect={(s) => (selected = s)} />
      <div class="exp-link">
        <a href="/experiments/{run.experiment_id}">open the full experiment view →</a>
      </div>
    {/if}
  </div>

  <div class="panel">
    <h2>Audit trail ({detail.audit.length})</h2>
    {#if detail.audit.length === 0}
      <div class="state">No audit events yet.</div>
    {:else}
      <div class="audit">
        {#each detail.audit as entry, i (i)}
          <div class="entry">
            <span class="mono ts">{fmtTs(entry.at_ns)}</span>
            <span class="mono event">{entry.event}</span>
            <span class="actor">{entry.actor ?? 'system'}</span>
            <span class="detail">{entry.detail ?? ''}</span>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  {#if selected && telemetry}
    <SpanDrawer span={selected} logs={telemetry.logs} onclose={() => (selected = null)} />
  {/if}
{/if}

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
  .exp-link {
    margin-top: 10px;
    font-size: 12.5px;
  }
  .audit {
    max-height: 420px;
    overflow-y: auto;
  }
  .entry {
    display: flex;
    gap: 10px;
    padding: 3px 0;
    font-size: 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
    align-items: baseline;
  }
  .ts {
    color: var(--text-faint);
    flex: 0 0 128px;
  }
  .event {
    flex: 0 0 150px;
  }
  .actor {
    color: var(--text-dim);
    flex: 0 0 110px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .detail {
    color: var(--text-dim);
    min-width: 0;
  }
</style>

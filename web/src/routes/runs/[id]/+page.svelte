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

  // --- T10: approval chain ---------------------------------------------------
  let approvalNote = $state('');
  let approvalBusy = $state(false);
  let approvalError = $state<string | null>(null);
  let approvalHint = $state<string | null>(null);

  let breakGlassOpen = $state(false);
  let breakGlassJustification = $state('');
  let breakGlassBusy = $state(false);
  let breakGlassError = $state<string | null>(null);

  // Approvers and admins may decide; only admins may break glass. The server
  // enforces both regardless — these only hide what the role cannot use.
  const canDecide = $derived(
    me
      ? !me.auth_required ||
          (me.authenticated && (me.role === 'approver' || me.role === 'admin'))
      : false
  );
  const isAdmin = $derived(
    me ? !me.auth_required || (me.authenticated && me.role === 'admin') : false
  );

  // Set by /runs/new when a gated start redirected here: shows the
  // "awaiting approval" notice while the run is still pending.
  const awaitingTier = $derived($page.url.searchParams.get('awaiting'));

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

  async function decide(approve: boolean) {
    if (approvalBusy) return;
    approvalBusy = true;
    approvalError = null;
    approvalHint = null;
    try {
      const note = approvalNote.trim() || undefined;
      const r = approve ? await api.approveRun(id, note) : await api.rejectRun(id, note);
      if (approve && r.state === 'pending_approval') {
        approvalHint = 'recorded — waiting for quorum';
      }
      approvalNote = '';
      // Polling continues on its own cadence and picks up the transition.
    } catch (e) {
      approvalError = String(e);
    } finally {
      approvalBusy = false;
    }
  }

  async function breakGlass() {
    if (breakGlassBusy) return;
    breakGlassError = null;
    if (breakGlassJustification.trim().length < 10) {
      breakGlassError = 'justification must be at least 10 characters';
      return;
    }
    breakGlassBusy = true;
    try {
      await api.breakGlass(id, breakGlassJustification.trim());
      breakGlassOpen = false;
      breakGlassJustification = '';
      // Polling continues on its own cadence; the run transitions to queued.
    } catch (e) {
      breakGlassError = String(e);
    } finally {
      breakGlassBusy = false;
    }
  }

  const tierClass = (t: string) => (t === 'T3' ? 'fail' : t === 'T2' ? 'warn' : 'neutral');

  /** Remaining approval TTL; `soon` (under 1h left, or lapsed) gets warn. */
  function expiry(ns: number): { text: string; soon: boolean } {
    const ms = ns / 1_000_000 - Date.now();
    if (ms <= 0) return { text: 'expired', soon: true };
    const m = Math.floor(ms / 60_000);
    if (m >= 60) return { text: `${Math.floor(m / 60)}h ${m % 60}m`, soon: false };
    return { text: `${m}m ${Math.floor((ms % 60_000) / 1000)}s`, soon: true };
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

  {#if detail.approval?.request}
    {@const req = detail.approval.request}
    {@const ttl = expiry(req.expires_at_ns)}
    <div class="panel" style="margin-bottom: 14px">
      <h2>
        Approval chain
        <span class="badge {tierClass(req.tier)}">{req.tier}</span>
        {#if req.break_glass}
          <span
            class="badge warn"
            title={req.break_glass_justification ?? 'no justification recorded'}
          >
            break-glass by {req.break_glass_by ?? '?'}
          </span>
        {/if}
        {#if req.consumed_at_ns !== null}
          <span class="badge neutral" title={fmtTs(req.consumed_at_ns)}>consumed</span>
        {/if}
      </h2>

      <div class="ap-meta">
        <div><span>Env</span><b>{req.env}</b></div>
        <div><span>Target</span><b>{req.target ?? '—'}</b></div>
        <div>
          <span>Requested by</span>
          <b title={fmtTs(req.requested_at_ns)}>
            {req.requested_by} · {fmtAgo(req.requested_at_ns)}
          </b>
        </div>
        <div>
          <span>Expires</span>
          <b class="mono" class:warn-text={ttl.soon} title={fmtTs(req.expires_at_ns)}>
            {ttl.text}
          </b>
        </div>
        <div><span>Quorum</span><b class="mono">{req.approved_count}/{req.quorum_required}</b></div>
        <div>
          <span>Pin</span>
          <b class="mono" title={req.pin_hash}>{req.pin_hash.slice(0, 16)}…</b>
        </div>
      </div>

      {#if detail.approval.decisions.length > 0}
        <div class="audit decisions">
          {#each detail.approval.decisions as d, i (i)}
            <div class="entry">
              <span class="mono ts">{fmtTs(d.decided_at_ns)}</span>
              <span class="event {d.decision === 'approved' ? 'ok-text' : 'fail-text'}">
                {d.decision === 'approved' ? '✓ approved' : '✗ rejected'}
              </span>
              <span class="actor">{d.approver}</span>
              <span class="detail">{d.note ?? ''}</span>
            </div>
          {/each}
        </div>
      {:else}
        <div class="state" style="padding: 8px 0">No decisions yet.</div>
      {/if}

      {#if run.state === 'pending_approval' && canDecide}
        <div class="ap-actions">
          <input type="text" placeholder="decision note…" bind:value={approvalNote} />
          <button class="btn" onclick={() => decide(true)} disabled={approvalBusy}>Approve</button>
          <button class="btn danger" onclick={() => decide(false)} disabled={approvalBusy}>
            Reject
          </button>
          {#if isAdmin && !breakGlassOpen}
            <button class="btn glass" onclick={() => (breakGlassOpen = true)}>Break glass</button>
          {/if}
        </div>
        {#if approvalError}
          <div class="err-line">{approvalError}</div>
        {/if}
        {#if approvalHint}
          <div class="ok-line">{approvalHint}</div>
        {/if}
      {/if}

      {#if run.state === 'pending_approval' && isAdmin && breakGlassOpen}
        <div class="glass-form">
          <p class="warn-copy">
            Break glass overrides the quorum and dispatches the run immediately. The
            override is audited and creates compliance debt — use it only when the
            normal approval path is unavailable.
          </p>
          <textarea
            rows="3"
            placeholder="justification (min 10 characters)…"
            bind:value={breakGlassJustification}
          ></textarea>
          <div class="ap-actions">
            <button
              class="btn glass"
              onclick={breakGlass}
              disabled={breakGlassBusy || breakGlassJustification.trim().length < 10}
            >
              {breakGlassBusy ? 'Overriding…' : 'Confirm break glass'}
            </button>
            <button class="btn" onclick={() => (breakGlassOpen = false)} disabled={breakGlassBusy}>
              Cancel
            </button>
            {#if breakGlassError}
              <span class="err-line">{breakGlassError}</span>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {/if}

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
  .notice {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    border-color: color-mix(in srgb, var(--warn) 55%, var(--border));
    font-size: 13px;
  }
  .ap-meta {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px 18px;
    margin-bottom: 12px;
  }
  .ap-meta span {
    display: block;
    color: var(--text-dim);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    margin-bottom: 4px;
  }
  .ap-meta b {
    font-weight: 600;
    font-size: 13.5px;
  }
  .decisions {
    max-height: 200px;
    margin-bottom: 12px;
  }
  .ok-text {
    color: var(--ok);
  }
  .fail-text {
    color: var(--fail);
  }
  .warn-text {
    color: var(--warn);
  }
  .ap-actions {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    margin-top: 10px;
  }
  .ap-actions input {
    flex: 1;
    min-width: 220px;
  }
  .btn {
    background: var(--bg-hover);
    border: 1px solid var(--border-strong);
    color: var(--text);
    border-radius: 6px;
    padding: 6px 14px;
    cursor: pointer;
    font-size: 13px;
  }
  .btn:hover {
    border-color: var(--accent-dim);
  }
  .btn.danger {
    border-color: var(--fail);
    color: var(--fail);
  }
  .btn.glass {
    border-color: var(--warn);
    color: var(--warn);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .glass-form {
    margin-top: 12px;
    padding: 12px;
    border: 1px solid color-mix(in srgb, var(--warn) 55%, var(--border));
    border-radius: 6px;
  }
  .warn-copy {
    color: var(--warn);
    font-size: 12.5px;
    line-height: 1.5;
    margin: 0 0 10px;
  }
  .glass-form textarea {
    width: 100%;
    resize: vertical;
  }
  .err-line {
    color: var(--fail);
    font-size: 12.5px;
    margin-top: 8px;
  }
  .ok-line {
    color: var(--ok);
    font-size: 12.5px;
    margin-top: 8px;
  }
</style>

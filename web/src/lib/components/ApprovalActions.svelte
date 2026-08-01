<script lang="ts">
  // T10 approval chain panel for a gated run: request metadata, recorded
  // decisions, and the actions — approve/reject with an optional note, plus
  // the admin-only two-step break-glass override (open the form, then
  // confirm with a justification).
  import { api, fmtAgo, fmtTs } from '$lib/api';
  import type { RunDetail, RunExecState } from '$lib/types';

  let {
    runId,
    runState,
    approval,
    canDecide,
    isAdmin
  }: {
    runId: string;
    runState: RunExecState;
    approval: RunDetail['approval'];
    canDecide: boolean;
    isAdmin: boolean;
  } = $props();

  let approvalNote = $state('');
  let approvalBusy = $state(false);
  let approvalError = $state<string | null>(null);
  let approvalHint = $state<string | null>(null);

  let breakGlassOpen = $state(false);
  let breakGlassJustification = $state('');
  let breakGlassBusy = $state(false);
  let breakGlassError = $state<string | null>(null);

  async function decide(approve: boolean) {
    if (approvalBusy) return;
    approvalBusy = true;
    approvalError = null;
    approvalHint = null;
    try {
      const note = approvalNote.trim() || undefined;
      const r = approve ? await api.approveRun(runId, note) : await api.rejectRun(runId, note);
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
      await api.breakGlass(runId, breakGlassJustification.trim());
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

{#if approval.request}
  {@const req = approval.request}
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

    {#if approval.decisions.length > 0}
      <div class="audit decisions">
        {#each approval.decisions as d, i (i)}
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

    {#if runState === 'pending_approval' && canDecide}
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

    {#if runState === 'pending_approval' && isAdmin && breakGlassOpen}
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

<style>
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

<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtAgo, fmtTs, shortId } from '$lib/api';
  import type { ApprovalQueueRow, ApprovalTier, MeResponse } from '$lib/types';

  let rows = $state<ApprovalQueueRow[] | null>(null);
  let error = $state<string | null>(null);
  let me = $state<MeResponse | null>(null);

  let openId = $state<string | null>(null);
  let notes: Record<string, string> = $state({});
  // Per-row action feedback: errors stick until the next attempt, the quorum
  // hint is transient (cleared on the next refresh that drops the row).
  let rowErrors: Record<string, string> = $state({});
  let rowHints: Record<string, string> = $state({});
  let busy = $state(false);

  // Approvers and admins may decide; the server enforces it regardless —
  // this only hides buttons the role cannot use.
  const canDecide = $derived(
    me
      ? !me.auth_required ||
          (me.authenticated && (me.role === 'approver' || me.role === 'admin'))
      : false
  );

  onMount(() => {
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
  });

  // Poll the queue every 5s for as long as the page is open; the cleanup
  // clears the pending tick, so no timers leak.
  $effect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    async function poll() {
      try {
        const r = await api.approvals();
        if (cancelled) return;
        rows = r.queue;
        error = null;
      } catch (e) {
        if (cancelled) return;
        // Keep the last snapshot on transient failures once loaded.
        if (!rows) error = String(e);
      }
      if (!cancelled) timer = setTimeout(poll, 5000);
    }

    void poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  });

  async function refresh() {
    try {
      const r = await api.approvals();
      rows = r.queue;
      error = null;
    } catch (e) {
      if (!rows) error = String(e);
    }
  }

  async function decide(runId: string, approve: boolean) {
    if (busy) return;
    busy = true;
    const errs = { ...rowErrors };
    delete errs[runId];
    rowErrors = errs;
    try {
      const note = notes[runId]?.trim() || undefined;
      const r = approve ? await api.approveRun(runId, note) : await api.rejectRun(runId, note);
      if (approve && r.state === 'pending_approval') {
        rowHints = { ...rowHints, [runId]: 'recorded — waiting for quorum' };
      } else {
        const hints = { ...rowHints };
        delete hints[runId];
        rowHints = hints;
      }
      await refresh();
    } catch (e) {
      rowErrors = { ...rowErrors, [runId]: String(e) };
    } finally {
      busy = false;
    }
  }

  function toggle(runId: string) {
    openId = openId === runId ? null : runId;
  }

  const tierClass = (t: ApprovalTier) => (t === 'T3' ? 'fail' : t === 'T2' ? 'warn' : 'neutral');

  /** Remaining TTL; `soon` (under 1h left, or lapsed) gets the warn tone. */
  function remaining(ns: number): { text: string; soon: boolean } {
    const ms = ns / 1_000_000 - Date.now();
    if (ms <= 0) return { text: 'expired', soon: true };
    const m = Math.floor(ms / 60_000);
    if (m >= 60) return { text: `${Math.floor(m / 60)}h ${m % 60}m`, soon: false };
    return { text: `${m}m ${Math.floor((ms % 60_000) / 1000)}s`, soon: true };
  }

  function prettyParams(raw: string | null): string {
    if (!raw) return '—';
    try {
      return JSON.stringify(JSON.parse(raw), null, 2);
    } catch {
      return raw;
    }
  }
</script>

<div class="page-head">
  <h1>Approvals</h1>
  <span class="sub">
    {rows ? `${rows.length} pending` : 'gated runs waiting on quorum'}
  </span>
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load the approvals queue: {error}</div>
  {:else if !rows}
    <div class="skeleton" style="height: 200px"></div>
  {:else if rows.length === 0}
    <div class="state">Nothing awaiting approval.</div>
  {:else}
    <table class="data">
      <thead>
        <tr>
          <th></th>
          <th>Definition</th>
          <th>Run</th>
          <th>Tier</th>
          <th>Env</th>
          <th>Requested</th>
          <th>Expires</th>
          <th>Quorum</th>
          {#if canDecide}
            <th>Note</th>
            <th></th>
          {/if}
        </tr>
      </thead>
      <tbody>
        {#each rows as row (row.run_id)}
          {@const ttl = remaining(row.expires_at_ns)}
          <tr class="clickable" onclick={() => toggle(row.run_id)}>
            <td style="color: var(--text-faint)">{openId === row.run_id ? '▾' : '▸'}</td>
            <td>
              {row.definition_name ?? '—'}
              {#if row.break_glass}
                <span
                  class="badge warn"
                  title="break-glass override by {row.break_glass_by ?? '?'}{row.break_glass_justification
                    ? `: ${row.break_glass_justification}`
                    : ''}"
                >
                  break-glass
                </span>
              {/if}
            </td>
            <td class="mono" style="color: var(--text-dim)">
              <a href="/runs/{row.run_id}" onclick={(e) => e.stopPropagation()}>
                {shortId(row.run_id)}
              </a>
            </td>
            <td><span class="badge {tierClass(row.tier)}">{row.tier}</span></td>
            <td>{row.env}</td>
            <td title={fmtTs(row.requested_at_ns)}>
              {row.requested_by} · {fmtAgo(row.requested_at_ns)}
            </td>
            <td class="mono" class:warn-text={ttl.soon} title={fmtTs(row.expires_at_ns)}>
              {ttl.text}
            </td>
            <td class="mono">{row.approved_count}/{row.quorum_required}</td>
            {#if canDecide}
              <td onclick={(e) => e.stopPropagation()}>
                <input type="text" placeholder="decision note…" bind:value={notes[row.run_id]} />
              </td>
              <td class="actions" onclick={(e) => e.stopPropagation()}>
                <button class="btn" onclick={() => decide(row.run_id, true)} disabled={busy}>
                  Approve
                </button>
                <button class="btn danger" onclick={() => decide(row.run_id, false)} disabled={busy}>
                  Reject
                </button>
              </td>
            {/if}
          </tr>
          {#if rowErrors[row.run_id] || rowHints[row.run_id]}
            <tr class="msg-row">
              <td colspan={canDecide ? 10 : 8}>
                {#if rowErrors[row.run_id]}
                  <span class="err-text">{rowErrors[row.run_id]}</span>
                {/if}
                {#if rowHints[row.run_id]}
                  <span class="hint-text">{rowHints[row.run_id]}</span>
                {/if}
              </td>
            </tr>
          {/if}
          {#if openId === row.run_id}
            <tr class="detail-row">
              <td colspan={canDecide ? 10 : 8}>
                <div class="detail">
                  <div>
                    <h3>Parameters</h3>
                    <pre class="mono params">{prettyParams(row.params_json)}</pre>
                  </div>
                  <div>
                    <h3>Pin hash</h3>
                    <div class="mono dim hash">{row.pin_hash}</div>
                    {#if row.target}
                      <h3>Target</h3>
                      <div class="dim">{row.target}</div>
                    {/if}
                  </div>
                </div>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .actions {
    display: flex;
    gap: 8px;
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
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .warn-text {
    color: var(--warn);
  }
  .msg-row td {
    background: var(--bg-raised);
    padding: 4px 12px;
    display: flex;
    gap: 12px;
  }
  .err-text {
    color: var(--fail);
    font-size: 12.5px;
  }
  .hint-text {
    color: var(--ok);
    font-size: 12.5px;
  }
  .detail-row td {
    background: var(--bg-raised);
  }
  .detail {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
    padding: 8px 4px;
  }
  .detail h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-dim);
    margin: 0 0 6px;
  }
  .params {
    margin: 0;
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--text-dim);
  }
  .dim {
    color: var(--text-dim);
    font-size: 12.5px;
  }
  .hash {
    word-break: break-all;
  }
</style>

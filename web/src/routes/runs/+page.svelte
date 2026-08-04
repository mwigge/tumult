<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { ACTIVE_RUN_STATES, api, fmtAgo, fmtDuration, fmtTs, shortId } from '$lib/api';
  import type { MeResponse, RunRow } from '$lib/types';
  import StatusBadge from '$lib/components/StatusBadge.svelte';

  // Client-side state filter: '' = all, 'active' = anything in
  // ACTIVE_RUN_STATES, otherwise one terminal state.
  const FILTERS = ['', 'active', 'passed', 'deviated', 'failed', 'aborted', 'orphaned', 'rollback_pending'];

  let rows = $state<RunRow[] | null>(null);
  let error = $state<string | null>(null);
  let me = $state<MeResponse | null>(null);
  let filter = $state('');

  // Global halt: two-step arm/confirm — the first click arms for 5s, the
  // second fires. No bare one-click kill switch.
  let haltArmed = $state(false);
  let halting = $state(false);
  let haltNote = $state<string | null>(null);
  let haltTimer: ReturnType<typeof setTimeout> | null = null;

  // Operators and up may launch runs; missing role counts as viewer. When
  // the daemon has no users (open local mode) anyone may run.
  const canRun = $derived(
    me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false
  );

  const shown = $derived(
    rows?.filter((r) =>
      filter === '' ? true : filter === 'active' ? ACTIVE_RUN_STATES.has(r.state) : r.state === filter
    ) ?? null
  );

  const activeCount = $derived(rows?.filter((r) => ACTIVE_RUN_STATES.has(r.state)).length ?? 0);

  onMount(() => {
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
    refresh();
    return () => {
      if (haltTimer) clearTimeout(haltTimer);
    };
  });

  async function refresh() {
    try {
      const r = await api.runs();
      rows = r.runs;
      error = null;
    } catch (e) {
      if (!rows) error = String(e);
    }
  }

  function disarm() {
    haltArmed = false;
    if (haltTimer) {
      clearTimeout(haltTimer);
      haltTimer = null;
    }
  }

  async function halt() {
    if (halting) return;
    if (!haltArmed) {
      haltArmed = true;
      haltTimer = setTimeout(disarm, 5000);
      return;
    }
    disarm();
    halting = true;
    haltNote = null;
    try {
      const r = await api.stopAllRuns();
      haltNote = `Halt requested — ${r.stopped} run${r.stopped === 1 ? '' : 's'} stopping${
        r.skipped_terminal > 0 ? `, ${r.skipped_terminal} already terminal` : ''
      }.`;
    } catch (e) {
      haltNote = `Halt failed: ${String(e)}`;
    } finally {
      halting = false;
    }
    await refresh();
  }

  function duration(r: RunRow): string {
    return fmtDuration(r.ended_at_ns !== null ? r.ended_at_ns - r.queued_at_ns : null);
  }
</script>

<div class="page-head">
  <h1>Runs</h1>
  <span class="sub">{shown ? `${shown.length} run${shown.length === 1 ? '' : 's'}` : 'newest first'}</span>
  <div class="controls">
    <div class="seg">
      {#each FILTERS as f (f)}
        <button class:active={filter === f} onclick={() => (filter = f)}>
          {f === '' ? 'all' : f}
        </button>
      {/each}
    </div>
    {#if canRun}
      {#if activeCount > 0}
        <button class:danger={haltArmed} class="halt" onclick={halt} disabled={halting}>
          {halting
            ? 'Halting…'
            : haltArmed
              ? `Confirm halt ${activeCount} active run${activeCount === 1 ? '' : 's'}`
              : 'Halt all'}
        </button>
      {/if}
      <button class="primary" onclick={() => goto('/runs/new')}>New run</button>
    {/if}
  </div>
</div>

{#if haltNote}
  <div class="panel halt-note">{haltNote}</div>
{/if}

<div class="panel">
  {#if error}
    <div class="state error">Failed to load runs: {error}</div>
  {:else if !shown}
    <div class="skeleton" style="height: 240px"></div>
  {:else if shown.length === 0}
    <div class="state">No runs match this filter.</div>
  {:else}
    <table class="data">
      <thead>
        <tr>
          <th>State</th><th>Definition</th><th>ID</th><th>Queued</th>
          <th>Duration</th><th>Rollback</th><th>Error</th>
        </tr>
      </thead>
      <tbody>
        {#each shown as row (row.id)}
          <tr class="clickable" onclick={() => goto(`/runs/${row.id}`)}>
            <td><StatusBadge status={row.state} /></td>
            <td>{row.definition_name ?? row.registry_id}</td>
            <td class="mono" style="color: var(--text-dim)">{shortId(row.id)}</td>
            <td title={fmtTs(row.queued_at_ns)}>{fmtAgo(row.queued_at_ns)}</td>
            <td class="mono">{duration(row)}</td>
            <td>
              {#if row.state === 'rollback_pending'}
                <span class="badge warn">rollback pending</span>
              {:else if row.rollback_status && row.rollback_status !== 'not_needed'}
                <span class="badge {row.rollback_status === 'failed' ? 'fail' : 'ok'}">
                  rollback {row.rollback_status}
                </span>
              {:else}
                <span style="color: var(--text-faint)">—</span>
              {/if}
            </td>
            <td class="err" title={row.error ?? ''}>{row.error ?? ''}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .controls {
    margin-left: auto;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }
  .halt {
    background: var(--bg-hover);
    border: 1px solid var(--border-strong);
    color: var(--text);
    border-radius: 6px;
    padding: 6px 14px;
    cursor: pointer;
    font-size: 13px;
  }
  .halt:hover {
    border-color: var(--fail);
  }
  .halt.danger {
    border-color: var(--fail);
    color: var(--fail);
    font-weight: 600;
  }
  .halt:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .halt-note {
    margin-bottom: 14px;
    font-size: 13px;
    color: var(--text-dim);
  }
  .err {
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
    font-size: 12px;
  }
</style>

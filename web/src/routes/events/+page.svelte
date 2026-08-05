<script lang="ts">
  import { api, fmtAgo, fmtTs, shortId } from '$lib/api';
  import type { RunEvent } from '$lib/types';

  // Event-type filter: '' = everything, otherwise one audit event name.
  const FILTERS: { value: string; label: string }[] = [
    { value: '', label: 'all events' },
    { value: 'enqueued', label: 'started' },
    { value: 'requested', label: 'approval needed' },
    { value: 'stop_requested', label: 'stops' },
    { value: 'aborted', label: 'aborted' },
    { value: 'passed', label: 'passed' },
    { value: 'failed', label: 'failed' }
  ];

  let rows = $state<RunEvent[] | null>(null);
  let error = $state<string | null>(null);
  let filter = $state('');

  // Poll every 5s while the page is open (same cadence as the approvals
  // queue); the cleanup clears the pending tick, so no timers leak.
  $effect(() => {
    const event = filter;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    async function poll() {
      try {
        const r = await api.events({ event, limit: '100' });
        if (cancelled) return;
        rows = r.events;
        error = null;
      } catch (e) {
        if (cancelled) return;
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

  /** Badge tone per event: failures red, stops/rejections amber, wins green. */
  function eventClass(event: string): string {
    switch (event) {
      case 'passed':
      case 'completed':
      case 'approved':
        return 'ok';
      case 'stop_requested':
      case 'requested':
      case 'rejected':
      case 'expired':
        return 'warn';
      case 'failed':
      case 'aborted':
      case 'orphaned':
        return 'fail';
      default:
        return 'neutral';
    }
  }
</script>

<div class="page-head">
  <h1>Events</h1>
  <span class="sub">{rows ? `${rows.length} recent` : 'run audit trail across all runs'}</span>
  <div class="controls">
    <div class="seg">
      {#each FILTERS as f (f.value)}
        <button class:active={filter === f.value} onclick={() => (filter = f.value)}>
          {f.label}
        </button>
      {/each}
    </div>
  </div>
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load events: {error}</div>
  {:else if !rows}
    <div class="skeleton" style="height: 240px"></div>
  {:else if rows.length === 0}
    <div class="state">No events yet — run something first.</div>
  {:else}
    <table class="data">
      <thead>
        <tr><th>When</th><th>Event</th><th>Definition</th><th>Run</th><th>Actor</th><th>Detail</th></tr>
      </thead>
      <tbody>
        {#each rows as e (e.run_id + e.at_ns)}
          <tr>
            <td title={fmtTs(e.at_ns)}>{fmtAgo(e.at_ns)}</td>
            <td><span class="badge {eventClass(e.event)}">{e.event}</span></td>
            <td>{e.definition_name ?? '—'}</td>
            <td class="mono" style="color: var(--text-dim)">
              <a href="/runs/{e.run_id}">{shortId(e.run_id)}</a>
            </td>
            <td>{e.actor ?? 'system'}</td>
            <td class="dim" title={e.detail ?? ''}>{e.detail ?? ''}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .controls {
    margin-left: auto;
  }
  .dim {
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
    font-size: 12px;
  }
</style>

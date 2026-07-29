<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { api, fmtAgo, fmtDuration, shortId } from '$lib/api';
  import type { Dimensions, ExperimentRow } from '$lib/types';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import RangeSwitch from '$lib/components/RangeSwitch.svelte';

  const filters = $derived({
    range: $page.url.searchParams.get('range') ?? '24h',
    outcome: $page.url.searchParams.get('outcome') ?? '',
    target: $page.url.searchParams.get('target') ?? '',
    fault: $page.url.searchParams.get('fault') ?? '',
    origin: $page.url.searchParams.get('origin') ?? '',
    q: $page.url.searchParams.get('q') ?? ''
  });

  let dims: Dimensions | null = $state(null);
  let rows: ExperimentRow[] | null = $state(null);
  let error: string | null = $state(null);

  onMount(() => {
    api.dimensions().then((d) => (dims = d)).catch(() => {});
  });

  $effect(() => {
    // Touch every filter so the effect re-runs on any change; debounce for
    // the free-text search.
    const params = { ...filters };
    let cancelled = false;
    rows = null;
    error = null;
    const t = setTimeout(() => {
      api
        .experiments(params)
        .then((r) => !cancelled && (rows = r.experiments))
        .catch((e) => !cancelled && (error = String(e)));
    }, 200);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  });

  function setFilter(key: string, value: string) {
    const params = new URLSearchParams($page.url.searchParams);
    if (value) params.set(key, value);
    else params.delete(key);
    goto(`?${params}`, { replaceState: true, keepFocus: true, noScroll: true });
  }
</script>

<div class="page-head">
  <h1>Experiments</h1>
  <span class="sub">{rows ? `${rows.length} run${rows.length === 1 ? '' : 's'}` : 'newest first'}</span>
  <div class="controls">
    <input
      type="search"
      placeholder="search name or id…"
      value={filters.q}
      oninput={(e) => setFilter('q', e.currentTarget.value)}
    />
    <select value={filters.outcome} onchange={(e) => setFilter('outcome', e.currentTarget.value)}>
      <option value="">all outcomes</option>
      {#each dims?.outcomes ?? [] as o (o)}
        <option value={o.toLowerCase()}>{o}</option>
      {/each}
      <option value="incomplete">incomplete</option>
    </select>
    <select value={filters.fault} onchange={(e) => setFilter('fault', e.currentTarget.value)}>
      <option value="">all faults</option>
      {#each dims?.faults ?? [] as f (f)}
        <option value={f}>{f}</option>
      {/each}
    </select>
    <select value={filters.target} onchange={(e) => setFilter('target', e.currentTarget.value)}>
      <option value="">all targets</option>
      {#each dims?.targets ?? [] as t (t)}
        <option value={t}>{t}</option>
      {/each}
    </select>
    <select value={filters.origin} onchange={(e) => setFilter('origin', e.currentTarget.value)}>
      <option value="">all origins</option>
      <option value="automated">automated</option>
      <option value="manual">manual</option>
    </select>
    <RangeSwitch value={filters.range} onchange={(r) => setFilter('range', r)} />
  </div>
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load experiments: {error}</div>
  {:else if !rows}
    <div class="skeleton" style="height: 240px"></div>
  {:else if rows.length === 0}
    <div class="state">No experiments match these filters.</div>
  {:else}
    <table class="data">
      <thead>
        <tr>
          <th>Status</th><th>Experiment</th><th>ID</th><th>Started</th>
          <th>Duration</th><th>Faults</th><th>Target</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row (row.id)}
          {#if row.origin === 'manual'}
            <!-- Manual records have no span waterfall; they live under /manual. -->
            <tr title="manual evidence record — see the Manual page">
              <td><StatusBadge status={row.status} /></td>
              <td>
                {row.name ?? '—'}
                <span class="badge neutral origin" title="review status: {row.review_status ?? 'unknown'}">manual</span>
              </td>
              <td class="mono" style="color: var(--text-dim)">{shortId(row.id)}</td>
              <td title={new Date(row.started_ns / 1e6).toISOString()}>{fmtAgo(row.started_ns)}</td>
              <td class="mono">{fmtDuration(row.duration_ns)}</td>
              <td class="mono" style="color: var(--warn)">{row.faults ?? ''}</td>
              <td class="mono">{row.target_system ?? '—'}</td>
            </tr>
          {:else}
            <tr class="clickable" onclick={() => goto(`/experiments/${row.id}`)}>
              <td><StatusBadge status={row.status} /></td>
              <td>{row.name ?? '—'}</td>
              <td class="mono" style="color: var(--text-dim)">{shortId(row.id)}</td>
              <td title={new Date(row.started_ns / 1e6).toISOString()}>{fmtAgo(row.started_ns)}</td>
              <td class="mono">{fmtDuration(row.duration_ns)}</td>
              <td class="mono" style="color: var(--warn)">{row.faults ?? ''}</td>
              <td class="mono">{row.target_system ?? '—'}</td>
            </tr>
          {/if}
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
  }
  .origin {
    margin-left: 6px;
    font-size: 10px;
  }
</style>

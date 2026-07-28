<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { MetricDefInfo, ReportFile } from '$lib/types';

  let reports: ReportFile[] | null = $state(null);
  let error: string | null = $state(null);

  let metrics: MetricDefInfo[] = $state([]);
  let selected = $state('');
  let generating = $state(false);
  let generateError: string | null = $state(null);
  let generated: string | null = $state(null);

  async function loadReports() {
    const r = await api.reports();
    reports = r.reports;
  }

  onMount(() => {
    loadReports().catch((e) => (error = String(e)));
    api
      .metrics()
      .then((m) => {
        metrics = m.metrics;
        if (metrics.length > 0) selected = metrics[0].name;
      })
      .catch(() => {});
  });

  async function generate() {
    if (!selected || generating) return;
    generating = true;
    generateError = null;
    try {
      const resp = await fetch('/api/reports/generate', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ metric: selected })
      });
      const body = await resp.json().catch(() => ({}));
      if (!resp.ok) throw new Error(body.error ?? `HTTP ${resp.status}`);
      generated = body.name;
      await loadReports();
    } catch (e) {
      generateError = String(e);
    } finally {
      generating = false;
    }
  }

  function fmtSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
</script>

<div class="page-head">
  <h1>Reports</h1>
  <span class="sub">metric digests — generated manually or by the daemon's scheduler</span>
</div>

<div class="panel" style="margin-bottom: 14px">
  <h2>Generate now</h2>
  <div class="genbar">
    <select bind:value={selected} disabled={metrics.length === 0}>
      {#each metrics as m (m.name)}
        <option value={m.name} title={m.description ?? ''}>{m.name}</option>
      {/each}
    </select>
    <button class="primary" onclick={generate} disabled={generating || !selected}>
      {generating ? 'Rendering…' : 'Generate'}
    </button>
    {#if generateError}
      <span class="state error" style="padding: 0">{generateError}</span>
    {/if}
  </div>
  {#if generated}
    <div class="result">
      <div class="result-head">
        <span class="mono">{generated}</span>
        <a href="/api/reports/{generated}" target="_blank" rel="noopener">open in new tab ↗</a>
      </div>
      <iframe src="/api/reports/{generated}" title="generated report" sandbox=""></iframe>
    </div>
  {/if}
</div>

<div class="panel">
  <h2>All digests</h2>
  {#if error}
    <div class="state error">Failed to load reports: {error}</div>
  {:else if !reports}
    <div class="skeleton" style="height: 160px"></div>
  {:else if reports.length === 0}
    <div class="state" style="text-align: left">
      <b>No digests yet.</b><br />
      Generate one above, or let the daemon produce them automatically:
      start kronikad with <code>KRONIKA_REPORT_INTERVAL=1h</code> and a digest
      is rendered into <code>&lt;db dir&gt;/reports/</code> every interval.
    </div>
  {:else}
    <table class="data">
      <thead>
        <tr><th>Digest</th><th>Origin</th><th>Rendered</th><th>Size</th><th></th></tr>
      </thead>
      <tbody>
        {#each reports as r (r.name)}
          <tr class="clickable" onclick={() => (generated = r.name)}>
            <td class="mono">{r.name}</td>
            <td>
              <span class="badge {r.name.startsWith('manual_') ? 'warn' : 'neutral'}">
                {r.name.startsWith('manual_') ? 'manual' : 'scheduled'}
              </span>
            </td>
            <td>{new Date(r.modified_s * 1000).toLocaleString()}</td>
            <td class="mono">{fmtSize(r.bytes)}</td>
            <td>
              <a href="/api/reports/{r.name}" target="_blank" rel="noopener">open ↗</a>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .genbar {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .result {
    margin-top: 14px;
  }
  .result-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 8px;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  iframe {
    width: 100%;
    height: 560px;
    border: 1px solid var(--border-strong);
    border-radius: 5px;
    background: #fff;
  }
</style>

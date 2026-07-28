<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { ReportFile } from '$lib/types';

  let reports: ReportFile[] | null = $state(null);
  let error: string | null = $state(null);

  onMount(() => {
    api
      .reports()
      .then((r) => (reports = r.reports))
      .catch((e) => (error = String(e)));
  });

  function fmtSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
</script>

<div class="page-head">
  <h1>Reports</h1>
  <span class="sub">digests rendered by the daemon's report scheduler</span>
</div>

<div class="panel">
  {#if error}
    <div class="state error">Failed to load reports: {error}</div>
  {:else if !reports}
    <div class="skeleton" style="height: 160px"></div>
  {:else if reports.length === 0}
    <div class="state" style="text-align: left">
      <b>No digests yet.</b><br />
      Automatic reporting is off by default. Start kronikad with
      <code>KRONIKA_REPORT_INTERVAL=1h</code> and a digest will be rendered into
      <code>&lt;db dir&gt;/reports/</code> every interval.
    </div>
  {:else}
    <table class="data">
      <thead>
        <tr><th>Digest</th><th>Rendered</th><th>Size</th><th></th></tr>
      </thead>
      <tbody>
        {#each reports as r (r.name)}
          <tr>
            <td class="mono">{r.name}</td>
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

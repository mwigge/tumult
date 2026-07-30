<script lang="ts">
  import { page } from '$app/stores';
  import { api, fmtAgo, fmtDuration, fmtTs } from '$lib/api';
  import type { ExperimentDetail, Span } from '$lib/types';
  import StatusBadge from '$lib/components/StatusBadge.svelte';
  import Waterfall from '$lib/components/Waterfall.svelte';
  import SpanDrawer from '$lib/components/SpanDrawer.svelte';

  const id = $derived($page.params.id ?? '');

  let detail: ExperimentDetail | null = $state(null);
  let error: string | null = $state(null);
  let selected: Span | null = $state(null);

  $effect(() => {
    let cancelled = false;
    detail = null;
    error = null;
    api
      .experiment(id)
      .then((d) => !cancelled && (detail = d))
      .catch((e) => !cancelled && (error = String(e)));
    return () => {
      cancelled = true;
    };
  });

  function sevClass(sev: string | null): string {
    const s = (sev ?? '').toUpperCase();
    if (s.startsWith('ERR') || s.startsWith('FATAL')) return 'fail';
    if (s.startsWith('WARN')) return 'warn';
    return '';
  }
</script>

{#if error}
  <div class="page-head"><h1>Experiment</h1></div>
  <div class="state error panel">{error}</div>
{:else if !detail}
  <div class="page-head"><h1>Experiment</h1></div>
  <div class="skeleton" style="height: 120px; margin-bottom: 14px"></div>
  <div class="skeleton" style="height: 320px"></div>
{:else}
  {@const exp = detail.experiment}
  <div class="page-head">
    <a href="/experiments" style="color: var(--text-dim)">← experiments</a>
    <h1>{exp.name ?? exp.id}</h1>
    <StatusBadge status={exp.status} />
    <span class="sub mono">{exp.id}</span>
  </div>

  <div class="grid" style="grid-template-columns: repeat(4, 1fr); margin-bottom: 14px">
    <div class="panel meta"><span>Started</span><b>{fmtTs(exp.started_ns)} ({fmtAgo(exp.started_ns)})</b></div>
    <div class="panel meta"><span>Duration</span><b class="mono">{exp.duration_ms ? `${exp.duration_ms}ms` : fmtDuration(exp.duration_ns)}</b></div>
    <div class="panel meta"><span>Deviations</span><b class="mono">{exp.deviations ?? '—'}</b></div>
    <div class="panel meta">
      <span>Target</span>
      <b class="mono">
        {exp.target_system ?? '—'}{exp.target_technology ? ` · ${exp.target_technology}` : ''}{exp.target_environment ? ` · ${exp.target_environment}` : ''}
      </b>
    </div>
  </div>

  <div class="panel" style="margin-bottom: 14px">
    <h2>Trace waterfall — {detail.spans.length} spans</h2>
    {#if detail.spans.length === 0}
      <div class="state">No spans recorded for this experiment.</div>
    {:else}
      <Waterfall spans={detail.spans} onselect={(s) => (selected = s)} />
    {/if}
  </div>

  <div class="grid cols-2">
    <div class="panel">
      <h2>Logs ({detail.logs.length})</h2>
      {#if detail.logs.length === 0}
        <div class="state">No logs correlated to this experiment.</div>
      {:else}
        <div class="logs">
          {#each detail.logs as log, i (i)}
            <div class="log">
              <span class="mono ts">{fmtTs(log.ts_ns)}</span>
              <span class="mono sev {sevClass(log.severity_text)}">{log.severity_text ?? 'LOG'}</span>
              <span>{log.body}</span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
    <div class="panel">
      <h2>Metric points ({detail.metrics.length})</h2>
      {#if detail.metrics.length === 0}
        <div class="state">No metric points tagged with this experiment's name.</div>
      {:else}
        <table class="data">
          <thead>
            <tr><th>Time</th><th>Metric</th><th>Value</th><th>Outcome</th></tr>
          </thead>
          <tbody>
            {#each detail.metrics as m, i (i)}
              <tr>
                <td class="mono">{fmtTs(m.ts_ns)}</td>
                <td class="mono">{m.metric_name}</td>
                <td class="mono">{m.value}</td>
                <td>{m.outcome_status ?? ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>

  {#if selected}
    <SpanDrawer span={selected} logs={detail.logs} onclose={() => (selected = null)} />
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
  .logs {
    max-height: 420px;
    overflow-y: auto;
  }
  .log {
    display: flex;
    gap: 10px;
    padding: 3px 0;
    font-size: 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
  }
  .ts {
    color: var(--text-faint);
    flex: 0 0 128px;
  }
  .sev {
    flex: 0 0 44px;
    color: var(--text-dim);
  }
  .sev.warn {
    color: var(--warn);
  }
  .sev.fail {
    color: var(--fail);
  }
</style>

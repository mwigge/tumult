<script lang="ts">
  import { page } from '$app/stores';
  import { api, fmtAgo, fmtDuration, fmtTs, shortId } from '$lib/api';
  import type { Span, TraceDetail } from '$lib/types';
  import Waterfall from '$lib/components/Waterfall.svelte';
  import SpanDrawer from '$lib/components/SpanDrawer.svelte';

  const id = $derived($page.params.id ?? '');

  let detail: TraceDetail | null = $state(null);
  let error: string | null = $state(null);
  let selected: Span | null = $state(null);
  // Aggregates computed at fetch time (array callbacks inside $derived lose
  // their contextual types under svelte-check).
  let started = $state(0);
  let ended = $state(0);
  let errors = $state(0);
  let experiment: Span | null = $state(null);

  $effect(() => {
    let cancelled = false;
    detail = null;
    error = null;
    api
      .trace(id)
      .then((d) => {
        if (cancelled) return;
        detail = d;
        const spans = d.spans;
        started = spans.length ? Math.min(...spans.map((s) => s.ts_ns)) : 0;
        ended = spans.length ? Math.max(...spans.map((s) => s.ts_ns + s.duration_ns)) : 0;
        errors = spans.filter((s) => s.status_code === 'Error').length;
        // tumult sets experiment_id only on the root span — any span can
        // provide it.
        experiment = spans.find((s) => s.experiment_id) ?? null;
      })
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
  <div class="page-head"><h1>Trace</h1></div>
  <div class="state error panel">{error}</div>
{:else if !detail}
  <div class="page-head"><h1>Trace</h1></div>
  <div class="skeleton" style="height: 120px; margin-bottom: 14px"></div>
  <div class="skeleton" style="height: 320px"></div>
{:else}
  <div class="page-head">
    <a href="/traces" style="color: var(--text-dim)">← traces</a>
    <h1 class="mono">{shortId(id)}</h1>
    {#if experiment?.experiment_id}
      <a href="/experiments/{experiment.experiment_id}">experiment {experiment.experiment_name ?? experiment.experiment_id}</a>
    {/if}
  </div>

  <div class="grid" style="grid-template-columns: repeat(4, 1fr); margin-bottom: 14px">
    <div class="panel meta"><span>Started</span><b>{fmtTs(started)} ({fmtAgo(started)})</b></div>
    <div class="panel meta"><span>Duration</span><b class="mono">{fmtDuration(ended - started)}</b></div>
    <div class="panel meta"><span>Spans</span><b class="mono">{detail.spans.length}</b></div>
    <div class="panel meta"><span>Errors</span><b class="mono" style="color: {errors > 0 ? 'var(--fail)' : 'inherit'}">{errors}</b></div>
  </div>

  <div class="panel" style="margin-bottom: 14px">
    <h2>Waterfall — {detail.spans.length} spans</h2>
    {#if detail.spans.length === 0}
      <div class="state">No spans recorded for this trace.</div>
    {:else}
      <Waterfall spans={detail.spans} onselect={(s) => (selected = s)} />
    {/if}
  </div>

  <div class="panel">
    <h2>Logs ({detail.logs.length})</h2>
    {#if detail.logs.length === 0}
      <div class="state">No logs correlated to this trace.</div>
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

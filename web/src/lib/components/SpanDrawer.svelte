<script lang="ts">
  // Right-hand drawer for one waterfall span: timing, status, attributes,
  // events and correlated logs (by span_id, else overlapping the span's
  // time window on the same trace).
  import type { LogRow, Span } from '$lib/types';
  import { fmtDuration, fmtTs } from '$lib/api';
  import StatusBadge from './StatusBadge.svelte';

  let {
    span,
    logs,
    onclose
  }: { span: Span; logs: LogRow[]; onclose: () => void } = $props();

  const correlated = $derived.by(() => {
    const direct = logs.filter((l) => l.span_id === span.span_id);
    const pool = direct.length > 0 ? direct : logs.filter(
      (l) =>
        l.trace_id === span.trace_id &&
        l.ts_ns >= span.ts_ns &&
        l.ts_ns <= span.ts_ns + span.duration_ns
    );
    return pool.slice(0, 50);
  });

  const attrs = $derived(Object.entries(span.span_attrs ?? {}));
  const events = $derived(Array.isArray(span.events) ? span.events : []);

  function sevClass(sev: string | null): string {
    const s = (sev ?? '').toUpperCase();
    if (s.startsWith('ERR') || s.startsWith('FATAL')) return 'fail';
    if (s.startsWith('WARN')) return 'warn';
    return '';
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onclose}></div>
<aside class="drawer">
  <header>
    <div>
      <div class="title mono">{span.span_name}</div>
      <StatusBadge status={span.status_code === 'Error' ? 'failed' : span.status_code === 'Ok' ? 'completed' : null} />
    </div>
    <button class="close" onclick={onclose} aria-label="close">×</button>
  </header>

  <section>
    <dl>
      <dt>Duration</dt>
      <dd class="mono">{fmtDuration(span.duration_ns)}</dd>
      <dt>Started</dt>
      <dd class="mono">{fmtTs(span.ts_ns)}</dd>
      <dt>Status</dt>
      <dd class="mono">{span.status_code}{span.status_message ? ` — ${span.status_message}` : ''}</dd>
      <dt>Span ID</dt>
      <dd class="mono">{span.span_id}</dd>
      <dt>Trace ID</dt>
      <dd class="mono">{span.trace_id}</dd>
      <dt>Service</dt>
      <dd class="mono">{span.service_name}</dd>
      {#if span.fault_type}
        <dt>Fault</dt>
        <dd class="mono">{span.fault_type}{span.fault_subtype ? ` / ${span.fault_subtype}` : ''}</dd>
      {/if}
    </dl>
  </section>

  {#if attrs.length > 0}
    <section>
      <h3>Attributes</h3>
      <table class="data">
        <tbody>
          {#each attrs as [k, v] (k)}
            <tr>
              <td class="mono dim">{k}</td>
              <td class="mono">{v}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/if}

  {#if events.length > 0}
    <section>
      <h3>Events</h3>
      <pre class="mono">{JSON.stringify(events, null, 2)}</pre>
    </section>
  {/if}

  <section>
    <h3>Correlated logs ({correlated.length})</h3>
    {#if correlated.length === 0}
      <div class="dim">No logs in this span's window.</div>
    {:else}
      {#each correlated as log, i (i)}
        <div class="log">
          <span class="mono sev {sevClass(log.severity_text)}">{log.severity_text ?? 'LOG'}</span>
          <span class="body">{log.body}</span>
        </div>
      {/each}
    {/if}
  </section>
</aside>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    z-index: 20;
  }
  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(480px, 92vw);
    background: var(--bg-raised);
    border-left: 1px solid var(--border-strong);
    z-index: 21;
    overflow-y: auto;
    padding: 18px;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 14px;
  }
  .title {
    font-size: 14px;
    margin-bottom: 6px;
    word-break: break-all;
  }
  .close {
    background: transparent;
    border: none;
    color: var(--text-dim);
    font-size: 22px;
    cursor: pointer;
    line-height: 1;
  }
  section {
    margin-bottom: 18px;
  }
  h3 {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    margin-bottom: 8px;
  }
  dl {
    display: grid;
    grid-template-columns: 88px 1fr;
    gap: 5px 10px;
    margin: 0;
    font-size: 12.5px;
  }
  dt {
    color: var(--text-dim);
  }
  dd {
    margin: 0;
    word-break: break-all;
  }
  pre {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 8px;
    font-size: 11px;
    overflow-x: auto;
  }
  .log {
    display: flex;
    gap: 8px;
    padding: 3px 0;
    font-size: 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
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
  .dim {
    color: var(--text-dim);
  }
</style>

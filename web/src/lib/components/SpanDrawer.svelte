<script lang="ts">
    // Right-hand drawer for one waterfall span: timing, status, attributes,
    // events and correlated logs (by span_id, else overlapping the span's
    // time window on the same trace). Attribute values offer click-to-filter
    // (⊕/⊖) onto the traces list; a run overlapping the span's window links
    // to its experiment page. It is a modal Drawer over a shade.
    import { goto } from '$app/navigation';
    import type { ExperimentWindow, LogRow, Span } from '#lib/types.js';
    import { api, fmtDuration, fmtTs } from '#lib/api.js';
    import { toneOf } from '#lib/tone.js';
    import Drawer from './Drawer.svelte';
    import StatusBadge from './StatusBadge.svelte';

    let { span, logs, onclose }: { span: Span; logs: LogRow[]; onclose: () => void } = $props();

    const title = $props.id();

    const correlated = $derived.by(() => {
        const direct = logs.filter((l) => l.span_id === span.span_id);
        const pool =
            direct.length > 0
                ? direct
                : logs.filter(
                      (l) =>
                          l.trace_id === span.trace_id &&
                          l.ts_ns >= span.ts_ns &&
                          l.ts_ns <= span.ts_ns + span.duration_ns,
                  );
        return pool.slice(0, 50);
    });

    const attrs = $derived(Object.entries(span.span_attrs ?? {}));
    const events = $derived(Array.isArray(span.events) ? span.events : []);

    // The experiment run overlapping this span's window (correlation leg),
    // when the span itself doesn't already carry experiment_id.
    let run: ExperimentWindow | null = $state(null);
    $effect(() => {
        let cancelled = false;
        run = null;
        if (!span.experiment_id) {
            api.experimentWindows(span.ts_ns, span.ts_ns + Math.max(span.duration_ns, 1))
                .then((w) => {
                    if (!cancelled) run = w.runs[0] ?? null;
                })
                .catch(() => {});
        }
        return () => {
            cancelled = true;
        };
    });
    const experimentId = $derived.by(() => span.experiment_id ?? run?.id ?? null);

    // Click-to-filter: hand the traces list an exact k=v attr predicate.
    function filterFor(k: string, v: string) {
        goto(`/traces?attr=${encodeURIComponent(`${k}=${v}`)}`);
    }
    function filterOut(k: string, v: string) {
        goto(`/traces?attr_not=${encodeURIComponent(`${k}=${v}`)}`);
    }
</script>

<Drawer labelledby={title} {onclose} backdrop>
    <header>
        <div>
            <div class="title mono" id={title}>{span.span_name}</div>
            <StatusBadge
                status={span.status_code === 'Error'
                    ? 'failed'
                    : span.status_code === 'Ok'
                      ? 'completed'
                      : null}
            />
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
            <dd class="mono">
                {span.status_code}{span.status_message ? ` — ${span.status_message}` : ''}
            </dd>
            <dt>Span ID</dt>
            <dd class="mono">{span.span_id}</dd>
            <dt>Trace ID</dt>
            <dd class="mono">{span.trace_id}</dd>
            <dt>Service</dt>
            <dd class="mono">{span.service_name}</dd>
            {#if span.fault_type}
                <dt>Fault</dt>
                <dd class="mono">
                    {span.fault_type}{span.fault_subtype ? ` / ${span.fault_subtype}` : ''}
                </dd>
            {/if}
            {#if experimentId}
                <dt>Experiment</dt>
                <dd class="mono">
                    <a href="/experiments/{encodeURIComponent(experimentId)}"
                        >{run?.name ?? span.experiment_name ?? experimentId} ↗</a
                    >
                </dd>
            {/if}
        </dl>
    </section>

    {#if attrs.length > 0}
        <section>
            <h3>Attributes</h3>
            <table class="data">
                <tbody>
                    {#each attrs as [k, v] (k)}
                        <tr class="attr-row">
                            <td class="mono dim">{k}</td>
                            <td class="mono">{v}</td>
                            <td class="facets">
                                <button
                                    title="filter traces for {k}={v}"
                                    onclick={() => filterFor(k, v)}>⊕</button
                                >
                                <button
                                    title="filter traces out {k}={v}"
                                    onclick={() => filterOut(k, v)}>⊖</button
                                >
                            </td>
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
                    <span class="mono sev {toneOf('severity', log.severity_text)}"
                        >{log.severity_text ?? 'LOG'}</span
                    >
                    <span class="body">{log.body}</span>
                    {#if log.trace_id}
                        <a
                            class="corr"
                            href="/traces/{encodeURIComponent(log.trace_id)}"
                            title="open trace">↗</a
                        >
                    {/if}
                </div>
            {/each}
        {/if}
    </section>
</Drawer>

<style>
    header {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        margin-bottom: var(--space-4);
    }
    .title {
        font-size: var(--font-size-xl);
        margin-bottom: var(--space-2);
        word-break: break-all;
    }
    section {
        margin-bottom: var(--space-5);
    }
    dl {
        display: grid;
        grid-template-columns: 88px 1fr;
        gap: var(--space-1) var(--space-3);
        margin: 0;
        font-size: var(--font-size-xs);
    }
    dd {
        margin: 0;
        word-break: break-all;
    }
    pre {
        background: var(--bg-panel);
        border: var(--border-width-sm) solid var(--border);
        border-radius: var(--radius-md);
        padding: var(--space-2);
        font-size: var(--font-size-3xs);
        overflow-x: auto;
    }
    .log {
        display: flex;
        gap: var(--space-2);
        padding: var(--space-1) 0;
        font-size: var(--font-size-xs);
        border-bottom: var(--border-width-sm) solid
            color-mix(in srgb, var(--border) 50%, transparent);
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
    /* Not the size of the global .dim: the size around it, or of .mono. */
    .dim:not(.mono) {
        font-size: inherit;
    }
    .facets {
        visibility: hidden;
        white-space: nowrap;
        border: none;
    }
    .attr-row:hover .facets {
        visibility: visible;
    }
    .facets button {
        background: transparent;
        border: var(--border-width-sm) solid var(--border);
        border-radius: var(--radius-md);
        color: var(--text-dim);
        cursor: pointer;
        font-size: var(--font-size-3xs);
        padding: 0 var(--space-1);
        margin-left: var(--space-1);
    }
    .facets button:hover {
        color: var(--accent);
        border-color: var(--accent);
    }
    .corr {
        color: var(--text-dim);
        text-decoration: none;
        margin-left: auto;
    }
    .corr:hover {
        color: var(--accent);
    }
</style>

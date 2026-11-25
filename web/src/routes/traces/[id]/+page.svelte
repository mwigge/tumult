<script lang="ts">
    import { page } from '$app/state';
    import { api, fmtAgo, fmtDuration, fmtTs, shortId } from '#lib/api.js';
    import { toneOf } from '#lib/tone.js';
    import type { Span, TraceDetail } from '#lib/types.js';
    import Waterfall from '#lib/components/Waterfall.svelte';
    import SpanDrawer from '#lib/components/SpanDrawer.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import MetaRow from '#lib/components/MetaRow.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';

    const id = $derived(page.params.id ?? '');

    let detail: TraceDetail | null = $state(null);
    let error: string | null = $state(null);
    let selected: Span | null = $state(null);
    // Aggregates computed at fetch time (array callbacks inside $derived lose
    // their contextual types under svelte-check).
    let started = $state(0);
    let ended = $state(0);
    let errors = $state(0);
    let experiment: Span | null = $state(null);
    // The trace is shown: it loaded and no error came.
    const shown = $derived(!error && detail !== null);

    $effect(() => {
        let cancelled = false;
        detail = null;
        error = null;
        api.trace(id)
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
</script>

<PageHeader
    title={shown ? shortId(id) : 'Trace'}
    mono={shown}
    back={shown ? { href: '/traces', label: 'traces' } : undefined}
>
    {#if shown && experiment?.experiment_id}
        <a href="/experiments/{experiment.experiment_id}"
            >experiment {experiment.experiment_name ?? experiment.experiment_id}</a
        >
    {/if}
</PageHeader>
{#if error}
    <State kind="error" panel>{error}</State>
{:else if !detail}
    <State kind="loading" height={120} style="margin-bottom: var(--space-4)" />
    <State kind="loading" height={320} />
{:else}
    <MetaRow>
        <Panel variant="meta" title="Started">
            <b>{fmtTs(started)} ({fmtAgo(started)})</b>
        </Panel>
        <Panel variant="meta" title="Duration">
            <b class="mono">{fmtDuration(ended - started)}</b>
        </Panel>
        <Panel variant="meta" title="Spans"><b class="mono">{detail.spans.length}</b></Panel>
        <Panel variant="meta" title="Errors">
            <b class="mono" style="color: {errors > 0 ? 'var(--fail)' : 'inherit'}">{errors}</b>
        </Panel>
    </MetaRow>

    <Panel title={`Waterfall — ${detail.spans.length} spans`} style="margin-bottom: var(--space-4)">
        {#if detail.spans.length === 0}
            <State kind="empty">No spans recorded for this trace.</State>
        {:else}
            <Waterfall spans={detail.spans} onselect={(s) => (selected = s)} />
        {/if}
    </Panel>

    <Panel title={`Logs (${detail.logs.length})`}>
        {#if detail.logs.length === 0}
            <State kind="empty">No logs correlated to this trace.</State>
        {:else}
            <div class="logs">
                {#each detail.logs as log, i (i)}
                    <div class="log">
                        <span class="mono ts">{fmtTs(log.ts_ns)}</span>
                        <span class="mono sev {toneOf('severity', log.severity_text)}"
                            >{log.severity_text ?? 'LOG'}</span
                        >
                        <span>{log.body}</span>
                    </div>
                {/each}
            </div>
        {/if}
    </Panel>

    {#if selected}
        <SpanDrawer span={selected} logs={detail.logs} onclose={() => (selected = null)} />
    {/if}
{/if}

<style>
    .logs {
        max-height: 420px;
        overflow-y: auto;
    }
    .log {
        display: flex;
        gap: var(--space-3);
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
</style>

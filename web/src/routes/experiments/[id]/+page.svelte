<script lang="ts">
    import { page } from '$app/state';
    import { api, fmtAgo, fmtDuration, fmtTs } from '#lib/api.js';
    import { experimentDurationNs } from '#lib/duration.js';
    import { toneOf } from '#lib/tone.js';
    import type { ExperimentDetail, Span } from '#lib/types.js';
    import Badge from '#lib/components/Badge.svelte';
    import EvidencePanels from '#lib/components/EvidencePanels.svelte';
    import StatusBadge from '#lib/components/StatusBadge.svelte';
    import Waterfall from '#lib/components/Waterfall.svelte';
    import SpanDrawer from '#lib/components/SpanDrawer.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';

    const id = $derived(page.params.id ?? '');

    let detail: ExperimentDetail | null = $state(null);
    let error: string | null = $state(null);
    let selected: Span | null = $state(null);
    // The experiment once it is shown: null while it loads and after an error.
    const shown = $derived.by(() => (error ? null : (detail?.experiment ?? null)));

    $effect(() => {
        let cancelled = false;
        detail = null;
        error = null;
        api.experiment(id)
            .then((d) => !cancelled && (detail = d))
            .catch((e) => !cancelled && (error = String(e)));
        return () => {
            cancelled = true;
        };
    });
</script>

<PageHeader
    title={shown ? (shown.name ?? shown.id) : 'Experiment'}
    back={shown ? { href: '/experiments', label: 'experiments' } : undefined}
>
    {#if shown}
        {@const exp = shown}
        <StatusBadge status={exp.status} />
        {#if exp.journal_only}
            <Badge
                tone="neutral"
                title="This run sent no trace. The lake holds its journal."
                label="journal only"
            />
        {/if}
        <span class="sub mono">{exp.id}</span>
    {/if}
</PageHeader>
{#if error}
    <div class="state error panel">{error}</div>
{:else if !detail}
    <div class="skeleton" style="height: 120px; margin-bottom: var(--space-3-5)"></div>
    <div class="skeleton" style="height: 320px"></div>
{:else}
    {@const exp = detail.experiment}
    {@const ranNs = experimentDurationNs(exp)}

    <div
        class="grid"
        style="grid-template-columns: repeat(4, 1fr); margin-bottom: var(--space-3-5)"
    >
        <Panel variant="meta" title="Started">
            <b>{fmtTs(exp.started_ns)} ({fmtAgo(exp.started_ns)})</b>
        </Panel>
        <div class="panel meta">
            <span>Duration</span><b class="mono">{fmtDuration(ranNs)}</b>
            {#if exp.duration_ns !== null && exp.duration_ns !== ranNs}
                <small title="The trace span also covers the wait that seals the run."
                    >trace span {fmtDuration(exp.duration_ns)}</small
                >
            {/if}
        </div>
        <Panel variant="meta" title="Deviations">
            <b class="mono">{exp.deviations ?? '—'}</b>
        </Panel>
        <Panel variant="meta" title="Target">
            <b class="mono">
                {exp.target_system ?? '—'}{exp.target_technology
                    ? ` · ${exp.target_technology}`
                    : ''}{exp.target_environment ? ` · ${exp.target_environment}` : ''}
            </b>
        </Panel>
    </div>

    <EvidencePanels evidence={detail.evidence ?? null} />

    <Panel
        title={`Trace waterfall — ${detail.spans.length} spans`}
        style="margin-bottom: var(--space-3-5)"
    >
        {#if exp.journal_only}
            <div class="state" data-testid="no-trace">
                This run sent no trace. What you see here comes from its journal.
            </div>
        {:else if detail.spans.length === 0}
            <div class="state">No spans recorded for this experiment.</div>
        {:else}
            <Waterfall spans={detail.spans} onselect={(s) => (selected = s)} />
        {/if}
    </Panel>

    <div class="grid cols-2">
        <Panel title={`Logs (${detail.logs.length})`}>
            {#if detail.logs.length === 0}
                <div class="state">No logs correlated to this experiment.</div>
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
        <Panel title={`Metric points (${detail.metrics.length})`}>
            {#if detail.metrics.length === 0}
                <div class="state">
                    No metric points with the name of this experiment in the time of this run.
                </div>
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
        </Panel>
    </div>

    {#if selected}
        <SpanDrawer span={selected} logs={detail.logs} onclose={() => (selected = null)} />
    {/if}
{/if}

<style>
    .meta small {
        display: block;
        color: var(--text-dim);
        font-size: var(--font-size-3xs);
        margin-top: var(--space-0-5);
    }
    .logs {
        max-height: 420px;
        overflow-y: auto;
    }
    .log {
        display: flex;
        gap: var(--space-2-5);
        padding: 3px 0;
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

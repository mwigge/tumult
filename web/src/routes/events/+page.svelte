<script lang="ts">
    import { api, fmtAgo, fmtTs, shortId } from '#lib/api.js';
    import type { RunEvent } from '#lib/types.js';
    import Badge from '#lib/components/Badge.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';

    // Event-type filter: '' = everything, otherwise one audit event name.
    const FILTERS: { value: string; label: string }[] = [
        { value: '', label: 'all events' },
        { value: 'enqueued', label: 'started' },
        { value: 'requested', label: 'approval needed' },
        { value: 'stop_requested', label: 'stops' },
        { value: 'aborted', label: 'aborted' },
        { value: 'passed', label: 'passed' },
        { value: 'failed', label: 'failed' },
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
</script>

<PageHeader title="Events" sub={rows ? `${rows.length} recent` : 'run audit trail across all runs'}>
    <div class="controls">
        <div class="seg">
            {#each FILTERS as f (f.value)}
                <button class:active={filter === f.value} onclick={() => (filter = f.value)}>
                    {f.label}
                </button>
            {/each}
        </div>
    </div>
</PageHeader>

<Panel>
    {#if error}
        <State kind="error">Failed to load events: {error}</State>
    {:else if !rows}
        <State kind="loading" height={240} />
    {:else if rows.length === 0}
        <State kind="empty">No events yet — run something first.</State>
    {:else}
        <table class="data">
            <thead>
                <tr
                    ><th>When</th><th>Event</th><th>Definition</th><th>Run</th><th>Actor</th><th
                        >Detail</th
                    ></tr
                >
            </thead>
            <tbody>
                {#each rows as e (e.run_id + e.at_ns)}
                    <tr>
                        <td title={fmtTs(e.at_ns)}>{fmtAgo(e.at_ns)}</td>
                        <td><Badge kind="event" value={e.event} /></td>
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
</Panel>

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
        font-size: var(--font-size-xs);
    }
</style>

<script lang="ts">
    // Scrollable audit trail for a run: timestamp, event, actor and detail per
    // entry, oldest first as returned by the API.
    import { fmtTs } from '#lib/api.js';
    import type { RunAuditEntry } from '#lib/types.js';
    import Panel from './Panel.svelte';
    import State from './State.svelte';

    let { audit }: { audit: RunAuditEntry[] } = $props();
</script>

<Panel title={`Audit trail (${audit.length})`}>
    {#if audit.length === 0}
        <State kind="empty">No audit events yet.</State>
    {:else}
        <div class="audit">
            {#each audit as entry, i (i)}
                <div class="entry">
                    <span class="mono ts">{fmtTs(entry.at_ns)}</span>
                    <span class="mono event">{entry.event}</span>
                    <span class="actor">{entry.actor ?? 'system'}</span>
                    <span class="detail">{entry.detail ?? ''}</span>
                </div>
            {/each}
        </div>
    {/if}
</Panel>

<style>
    .audit {
        max-height: 420px;
        overflow-y: auto;
    }
    .entry {
        display: flex;
        gap: var(--space-3);
        padding: var(--space-1) 0;
        font-size: var(--font-size-xs);
        border-bottom: var(--border-width-sm) solid
            color-mix(in srgb, var(--border) 50%, transparent);
        align-items: baseline;
    }
    .event {
        flex: 0 0 150px;
    }
    .actor {
        color: var(--text-dim);
        flex: 0 0 110px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .detail {
        color: var(--text-dim);
        min-width: 0;
    }
</style>

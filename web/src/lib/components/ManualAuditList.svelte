<script lang="ts">
    // The audit trail of a record or a plan, oldest row first.
    import { fmtTs } from '#lib/api.js';
    import type { ManualAuditEntry } from '#lib/types.js';

    let { rows }: { rows: ManualAuditEntry[] } = $props();
</script>

<ol class="audit">
    {#each rows as a (a.id)}
        <li>
            <span class="mono">{a.action}</span> by {a.changed_by}
            <span class="dim">· {fmtTs(a.changed_at_ns)}</span>
        </li>
    {/each}
</ol>

<style>
    .audit {
        margin: 0;
        padding-left: 18px;
    }
    .audit li {
        margin-bottom: 3px;
    }
    .dim {
        color: var(--text-faint);
        font-size: 12px;
    }
</style>

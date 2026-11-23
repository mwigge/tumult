<script lang="ts">
    // The actions an exercise leaves, each with the person who owns it, the
    // day it is due, how much it matters, and the run or the record that
    // retested it. The actions of a TLPT record are its remediation plan.
    import { emptyAction, RETEST_KINDS, SEVERITIES, type ActionRow } from '#lib/manual.js';

    let { rows = $bindable() }: { rows: ActionRow[] } = $props();
</script>

{#if rows.length === 0}
    <div class="dim">No actions yet.</div>
{:else}
    <div class="grid-rows actions-grid" aria-hidden="true">
        <span>#</span><span>Title</span><span>Owner</span><span>Due</span><span>Severity</span><span
            >Ticket</span
        ><span>Retested by</span><span>Retest id</span><span></span>
    </div>
    {#each rows as row, i (i)}
        <div class="grid-rows actions-grid">
            <span class="n">{i + 1}</span>
            <input type="text" aria-label="Action {i + 1} title" bind:value={row.title} />
            <input type="text" aria-label="Action {i + 1} owner" bind:value={row.owner} />
            <input type="date" aria-label="Action {i + 1} due date" bind:value={row.due} />
            <select aria-label="Action {i + 1} severity" bind:value={row.severity}>
                <option value="">not set</option>
                {#each SEVERITIES as s (s)}<option value={s}>{s}</option>{/each}
            </select>
            <input type="text" aria-label="Action {i + 1} ticket" bind:value={row.ticket} />
            <select aria-label="Action {i + 1} retest kind" bind:value={row.retest_kind}>
                <option value="">not retested</option>
                {#each RETEST_KINDS as k (k)}<option value={k}>{k}</option>{/each}
            </select>
            <input
                type="text"
                aria-label="Action {i + 1} retest id"
                placeholder="run or record id"
                bind:value={row.retest_id}
            />
            <button
                class="btn"
                aria-label="Remove action {i + 1}"
                onclick={() => (rows = rows.filter((_, j) => j !== i))}>×</button
            >
        </div>
    {/each}
{/if}
<button class="btn add" onclick={() => (rows = [...rows, emptyAction()])}>Add action</button>

<style>
    .actions-grid {
        grid-template-columns: 22px 2fr 1fr 1fr 0.9fr 1fr 1fr 1.1fr 34px;
    }
</style>

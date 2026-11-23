<script lang="ts">
    // The actions an exercise leaves, each with the person who owns it and
    // the day it is due.
    import { emptyAction, type ActionRow } from '#lib/manual.js';

    let { rows = $bindable() }: { rows: ActionRow[] } = $props();
</script>

{#if rows.length === 0}
    <div class="dim">No actions yet.</div>
{:else}
    <div class="grid-rows actions-grid" aria-hidden="true">
        <span>#</span><span>Title</span><span>Owner</span><span>Due</span><span>Ticket</span><span
        ></span>
    </div>
    {#each rows as row, i (i)}
        <div class="grid-rows actions-grid">
            <span class="n">{i + 1}</span>
            <input type="text" aria-label="Action {i + 1} title" bind:value={row.title} />
            <input type="text" aria-label="Action {i + 1} owner" bind:value={row.owner} />
            <input type="date" aria-label="Action {i + 1} due date" bind:value={row.due} />
            <input type="text" aria-label="Action {i + 1} ticket" bind:value={row.ticket} />
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
        grid-template-columns: 22px 2.4fr 1fr 1fr 1.2fr 34px;
    }
</style>

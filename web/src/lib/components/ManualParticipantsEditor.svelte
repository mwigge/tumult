<script lang="ts">
    // The people who took part in an exercise, and their roles.
    import { emptyParticipant, type ParticipantRow } from '#lib/manual.js';

    let { rows = $bindable() }: { rows: ParticipantRow[] } = $props();
</script>

{#if rows.length === 0}
    <div class="dim">No participants yet.</div>
{:else}
    {#each rows as row, i (i)}
        <div class="grid-rows people">
            <span class="n">{i + 1}</span>
            <input
                type="text"
                aria-label="Participant {i + 1} name"
                placeholder="name"
                bind:value={row.name}
            />
            <input
                type="text"
                aria-label="Participant {i + 1} role"
                placeholder="role"
                bind:value={row.role}
            />
            <button
                class="btn"
                aria-label="Remove participant {i + 1}"
                onclick={() => (rows = rows.filter((_, j) => j !== i))}>×</button
            >
        </div>
    {/each}
{/if}
<button class="btn add" onclick={() => (rows = [...rows, emptyParticipant()])}
    >Add participant</button
>

<style>
    .people {
        grid-template-columns: 22px 1.3fr 1fr 34px;
    }
</style>

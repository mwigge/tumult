<script lang="ts">
    // The steps of an exercise, in the order they were taken: what was
    // done, by whom, from when to when, and how it ended.
    import { emptyStep, STEP_RESULTS, type StepRow } from '#lib/manual.js';

    let { rows = $bindable() }: { rows: StepRow[] } = $props();
</script>

{#if rows.length === 0}
    <div class="dim">No steps yet.</div>
{:else}
    <div class="grid-rows steps" aria-hidden="true">
        <span>#</span><span>Title</span><span>Actor</span><span>Start</span><span>End</span><span
            >Result</span
        ><span>Note</span><span></span>
    </div>
    {#each rows as row, i (i)}
        <div class="grid-rows steps">
            <span class="n">{i + 1}</span>
            <input type="text" aria-label="Step {i + 1} title" bind:value={row.title} />
            <input type="text" aria-label="Step {i + 1} actor" bind:value={row.actor} />
            <input
                type="datetime-local"
                step="1"
                aria-label="Step {i + 1} start"
                bind:value={row.started}
            />
            <input
                type="datetime-local"
                step="1"
                aria-label="Step {i + 1} end"
                bind:value={row.ended}
            />
            <select aria-label="Step {i + 1} result" bind:value={row.result}>
                <option value="">none</option>
                {#each STEP_RESULTS as r (r)}<option value={r}>{r}</option>{/each}
            </select>
            <input type="text" aria-label="Step {i + 1} note" bind:value={row.note} />
            <button
                class="btn"
                aria-label="Remove step {i + 1}"
                onclick={() => (rows = rows.filter((_, j) => j !== i))}>×</button
            >
        </div>
    {/each}
{/if}
<button class="btn add" onclick={() => (rows = [...rows, emptyStep()])}>Add step</button>

<style>
    .steps {
        grid-template-columns: 22px 2fr 1fr 1.5fr 1.5fr 0.9fr 1.6fr 34px;
    }
</style>

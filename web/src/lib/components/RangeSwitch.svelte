<script lang="ts">
    // Segmented time-range control: 24h / 7d / 14d unless a page passes
    // its own `ranges`.
    import { SHORT_RANGES } from '#lib/ranges.js';

    let {
        value,
        onchange,
        ranges = SHORT_RANGES,
    }: { value: string; onchange: (range: string) => void; ranges?: string[] } = $props();
</script>

<div class="seg" role="group" aria-label="time range">
    {#each ranges as r (r)}
        <button class:active={value === r} onclick={() => onchange(r)}>{r}</button>
    {/each}
</div>

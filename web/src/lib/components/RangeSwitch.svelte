<script lang="ts">
    // Segmented time-range control: 24h / 7d / 14d unless a page passes
    // its own `ranges`.
    import { SHORT_RANGES } from '#lib/ranges.js';
    import Segmented from './Segmented.svelte';

    let {
        value,
        onchange,
        ranges = SHORT_RANGES,
    }: { value: string; onchange: (range: string) => void; ranges?: string[] } = $props();
</script>

<Segmented label="time range" options={ranges} {value} {onchange} />

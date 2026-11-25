<script lang="ts">
    // The row of meta tiles at the head of a run, a trace or an
    // experiment: four tiles in a row, and two below the sm breakpoint of
    // theme.css. Each tile is a Panel with variant="meta".
    import type { Snippet } from 'svelte';

    let { children }: { children: Snippet } = $props();
</script>

<div class="grid">
    {@render children()}
</div>

<style>
    .grid {
        grid-template-columns: repeat(4, 1fr);
        margin-bottom: var(--space-3-5);
    }
    @media (max-width: 800px) {
        .grid {
            grid-template-columns: repeat(2, 1fr);
        }
    }
</style>

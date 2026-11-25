<script lang="ts">
    // The panel of Tabs.svelte of the same `id`, named by the chosen tab. A
    // key reaches it, as its first content may not take the focus.
    import type { Snippet } from 'svelte';
    import { panelId, tabId } from '#lib/tabs.js';

    let { id, value, children }: { id: string; value: string; children?: Snippet } = $props();
</script>

<div role="tabpanel" id={panelId(id)} aria-labelledby={tabId(id, value)} tabindex="0">
    {@render children?.()}
</div>

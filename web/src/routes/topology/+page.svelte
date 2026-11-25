<script lang="ts">
    // The topology graph moved into the Star Map. Old links keep working:
    // this page forwards to /starmap and carries the time range along.
    import { onMount } from 'svelte';
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import State from '#lib/components/State.svelte';

    onMount(() => {
        const range = page.url.searchParams.get('range');
        goto(range ? `/starmap?range=${encodeURIComponent(range)}` : '/starmap', {
            replace: true,
        });
    });
</script>

<State kind="empty">The topology graph is now the <a href="/starmap">Star Map</a>.</State>

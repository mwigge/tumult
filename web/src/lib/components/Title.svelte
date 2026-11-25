<script lang="ts">
    // The title of the browser tab: the name of the page, then Krönika.
    // PageHeader sets it for every page with a head. A page with no head,
    // such as the login page, uses Title itself.
    let { name }: { name: string } = $props();
</script>

<svelte:head>
    <title>{name} · Krönika</title>
</svelte:head>

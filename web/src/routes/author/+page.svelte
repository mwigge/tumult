<script lang="ts">
    import { onMount } from 'svelte';
    import { api } from '#lib/api.js';
    import CatalogBrowser from '#lib/components/CatalogBrowser.svelte';
    import type { CatalogResponse } from '#lib/types.js';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';

    let catalog = $state<CatalogResponse | null>(null);
    let loadError = $state<string | null>(null);

    onMount(() => {
        api.catalog()
            .then((c) => (catalog = c))
            .catch((e) => (loadError = String(e)));
    });
</script>

<PageHeader title="Author an experiment" sub="pick a fault from the live plugin catalog" />

{#if loadError}
    <State kind="error">Failed to load the fault catalog: {loadError}</State>
{:else if !catalog}
    <State kind="loading" height={200} />
{:else if catalog.action_count === 0}
    <Panel>
        <State kind="empty">
            The fault catalog is empty — the daemon found no plugins. Plugin discovery looks in
            <span class="mono">./plugins</span>, <span class="mono">~/.tumult/plugins</span>, and
            the
            <span class="mono">TUMULT_PLUGIN_PATH</span> environment variable.
        </State>
    </Panel>
{:else}
    <CatalogBrowser domains={catalog.domains} />
{/if}

<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import CatalogBrowser from '$lib/components/CatalogBrowser.svelte';
  import type { CatalogResponse } from '$lib/types';

  let catalog = $state<CatalogResponse | null>(null);
  let loadError = $state<string | null>(null);

  onMount(() => {
    api
      .catalog()
      .then((c) => (catalog = c))
      .catch((e) => (loadError = String(e)));
  });
</script>

<div class="page-head">
  <h1>Author an experiment</h1>
  <span class="sub">pick a fault from the live plugin catalog</span>
</div>

{#if loadError}
  <div class="state error">Failed to load the fault catalog: {loadError}</div>
{:else if !catalog}
  <div class="skeleton" style="height: 200px"></div>
{:else if catalog.action_count === 0}
  <div class="panel">
    <div class="state">
      The fault catalog is empty — the daemon found no plugins. Plugin discovery looks in
      <span class="mono">./plugins</span>, <span class="mono">~/.tumult/plugins</span>, and the
      <span class="mono">TUMULT_PLUGIN_PATH</span> environment variable.
    </div>
  </div>
{:else}
  <CatalogBrowser domains={catalog.domains} />
{/if}

<script lang="ts">
  import { goto } from '$app/navigation';
  import type { CatalogAction, CatalogDomain } from '$lib/types';

  let { domains }: { domains: CatalogDomain[] } = $props();

  let q = $state('');
  let domain = $state('');

  const filtered: CatalogDomain[] = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    return domains
      .filter((d) => domain === '' || d.domain === domain)
      .map((d) => ({
        ...d,
        actions: d.actions.filter(
          (a) =>
            needle === '' ||
            a.name.toLowerCase().includes(needle) ||
            a.description.toLowerCase().includes(needle) ||
            a.plugin.toLowerCase().includes(needle)
        )
      }))
      .filter((d) => d.actions.length > 0);
  });

  function use(a: CatalogAction) {
    void goto(`/author/new?plugin=${encodeURIComponent(a.plugin)}&action=${encodeURIComponent(a.name)}`);
  }
</script>

<div class="filters">
  <input type="search" placeholder="Search actions, plugins, descriptions…" bind:value={q} />
  <select bind:value={domain}>
    <option value="">All domains</option>
    {#each domains as d (d.domain)}
      <option value={d.domain}>{d.label}</option>
    {/each}
  </select>
</div>

{#if filtered.length === 0}
  <div class="state">No catalog actions match.</div>
{:else}
  {#each filtered as d (d.domain)}
    <div class="panel" style="margin-bottom: 14px">
      <h2>{d.label} <span class="count">({d.actions.length})</span></h2>
      <table class="data">
        <thead>
          <tr><th>Action</th><th>Kind</th><th>Description</th><th></th></tr>
        </thead>
        <tbody>
          {#each d.actions as a (`${a.plugin}::${a.name}`)}
            <tr class="clickable" onclick={() => use(a)}>
              <td class="mono">{a.name}</td>
              <td><span class="badge {a.kind === 'action' ? 'warn' : 'neutral'}">{a.kind}</span></td>
              <td style="color: var(--text-dim)">{a.description}</td>
              <td style="color: var(--accent)">Use →</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/each}
{/if}

<style>
  .filters {
    display: flex;
    gap: 10px;
    margin-bottom: 14px;
  }
  .filters input {
    flex: 1;
  }
  h2 .count {
    color: var(--text-dim);
    font-weight: normal;
  }
</style>

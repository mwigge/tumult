<script lang="ts">
    import { goto } from '$app/navigation';
    import type { CatalogAction, CatalogDomain } from '#lib/types.js';
    import Badge from './Badge.svelte';
    import Panel from './Panel.svelte';
    import State from './State.svelte';
    import { onRow } from '#lib/rows.js';

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
                        a.plugin.toLowerCase().includes(needle),
                ),
            }))
            .filter((d) => d.actions.length > 0);
    });

    /** The form that authors an experiment with the action `a`. */
    const formHref = (a: CatalogAction) =>
        `/author/new?plugin=${encodeURIComponent(a.plugin)}&action=${encodeURIComponent(a.name)}`;
</script>

<div class="filters">
    <input
        type="search"
        aria-label="Search actions"
        placeholder="Search actions, plugins, descriptions…"
        bind:value={q}
    />
    <select aria-label="Domain" bind:value={domain}>
        <option value="">All domains</option>
        {#each domains as d (d.domain)}
            <option value={d.domain}>{d.label}</option>
        {/each}
    </select>
</div>

{#if filtered.length === 0}
    <State kind="empty">No catalog actions match.</State>
{:else}
    {#each filtered as d (d.domain)}
        <Panel style="margin-bottom: var(--space-4)" scroll>
            <h2>{d.label} <span class="count">({d.actions.length})</span></h2>
            <table class="data">
                <thead>
                    <tr><th>Action</th><th>Kind</th><th>Description</th><th></th></tr>
                </thead>
                <tbody>
                    {#each d.actions as a (`${a.plugin}::${a.name}`)}
                        <tr class="clickable" onclick={onRow(() => void goto(formHref(a)))}>
                            <td class="mono"><a class="row-link" href={formHref(a)}>{a.name}</a></td
                            >
                            <td><Badge kind="catalog" value={a.kind} /></td>
                            <td style="color: var(--text-dim)">{a.description}</td>
                            <td style="color: var(--accent)">Use →</td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        </Panel>
    {/each}
{/if}

<style>
    .filters {
        display: flex;
        gap: var(--space-3);
        margin-bottom: var(--space-4);
    }
    .filters input {
        flex: 1;
    }
    h2 .count {
        color: var(--text-dim);
        font-weight: normal;
    }
</style>

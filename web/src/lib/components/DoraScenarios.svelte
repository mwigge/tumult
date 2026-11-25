<script lang="ts">
    // The plan scenarios of RTS 2024/1774 for each critical or important
    // function: passed, failed, overdue or missing. A missing scenario is
    // a gap, not a pass, so it is said as such and never left blank.
    import type { DoraScenarioFunction, DoraScenarios } from '#lib/types.js';
    import Badge from './Badge.svelte';
    import Panel from './Panel.svelte';
    import State from './State.svelte';

    let { scenarios }: { scenarios: DoraScenarios } = $props();

    const cellOf = (f: DoraScenarioFunction, id: string) => f.scenarios.find((s) => s.id === id);
    // The ids in words: "a", "a and b" or "a, b and c".
    const listOf = (ids: string[]) =>
        ids.length < 2 ? ids.join('') : `${ids.slice(0, -1).join(', ')} and ${ids.at(-1)}`;
    const one = $derived(scenarios.rows.length === 1);
</script>

<Panel tag="section" title="Plan scenarios per function" data-testid="dora-scenarios" scroll>
    <p>
        The scenarios of
        <a href={scenarios.source_url} rel="external noopener noreferrer" target="_blank"
            >{scenarios.source}</a
        >
        that each critical or important function was tested against. {one ? 'Row' : 'Rows'}
        <span class="mono">{listOf(scenarios.rows)}</span>
        {one ? 'is' : 'are'} scored from them.
    </p>
    {#if scenarios.functions.length === 0}
        <State kind="empty">
            No critical or important function on record. Give your functions a
            <span class="mono">tier</span> in org.yaml.
        </State>
    {:else}
        <table class="data">
            <caption>Plan scenarios by function</caption>
            <thead>
                <tr>
                    <th scope="col">Scenario</th>
                    {#each scenarios.functions as f (f.entity_id)}
                        <th scope="col">{f.name} <span>{f.tier}</span></th>
                    {/each}
                </tr>
            </thead>
            <tbody>
                {#each scenarios.catalogue as s (s.id)}
                    <tr data-scenario={s.id}>
                        <th scope="row">
                            {s.title}
                            <div class="mono">{s.id} · {s.cite}</div>
                        </th>
                        {#each scenarios.functions as f (f.entity_id)}
                            {@const cell = cellOf(f, s.id)}
                            <td>
                                {#if cell}
                                    <Badge kind="scenario" value={cell.state} />
                                    {#if cell.last_tested_at}
                                        <div class="mono">tested {cell.last_tested_at}</div>
                                    {/if}
                                {:else}
                                    –
                                {/if}
                            </td>
                        {/each}
                    </tr>
                {/each}
            </tbody>
            <tfoot>
                <tr data-counts>
                    <th scope="row">Passed in {scenarios.window_days} days</th>
                    {#each scenarios.functions as f (f.entity_id)}
                        <td class="mono">{f.counts.passed} of {f.scenarios.length}</td>
                    {/each}
                </tr>
            </tfoot>
        </table>
    {/if}
    <div class="foot">{scenarios.note}</div>
</Panel>

<style>
    caption {
        text-align: left;
        font-weight: var(--font-weight-strong);
        padding-bottom: var(--space-2, 8px);
    }
</style>

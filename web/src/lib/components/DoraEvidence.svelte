<script lang="ts">
    // The evidence behind one requirement row, as a drawer: every function
    // of the row with its status and the records that scored it.
    import { api } from '#lib/api.js';
    import { pct } from '#lib/dora.js';
    import type { DoraArticleReport, DoraExpectation, DoraRequirement } from '#lib/types.js';
    import Badge from './Badge.svelte';

    let { row, lei, onclose }: { row: DoraExpectation; lei: string; onclose: () => void } =
        $props();

    let doc = $state<DoraArticleReport | null>(null);
    let error = $state<string | null>(null);
    let closer = $state<HTMLButtonElement | null>(null);

    $effect(() => {
        const wanted = row;
        let cancelled = false;
        doc = null;
        error = null;
        api.doraArticle(wanted.article, lei)
            .then((d) => !cancelled && (doc = d))
            .catch((e) => !cancelled && (error = String(e)));
        return () => {
            cancelled = true;
        };
    });

    $effect(() => {
        closer?.focus();
    });

    const requirement = $derived<DoraRequirement | null>(
        doc?.requirements.find((r) => r.id === row.id) ?? null,
    );

    const day = (ns: number) => new Date(ns / 1e6).toISOString().slice(0, 10);

    function onkeydown(event: KeyboardEvent) {
        if (event.key === 'Escape') onclose();
    }
</script>

<svelte:window {onkeydown} />

<div
    class="drawer"
    role="dialog"
    aria-modal="false"
    aria-label="Evidence behind {row.cite}"
    data-testid="evidence-drawer"
>
    <div class="top">
        <div>
            <div class="mono">{row.cite}</div>
            <div class="title">{row.title}</div>
            <div class="dim mono">{row.id}</div>
        </div>
        <button bind:this={closer} onclick={onclose}>Close</button>
    </div>
    {#if error}
        <div class="state error">Failed to load the evidence: {error}</div>
    {:else if !doc}
        <div class="skeleton" style="height: 120px"></div>
    {:else if !requirement}
        <div class="state">This row is not part of Art. {row.article} for this profile.</div>
    {:else}
        <div class="dim">
            expected {requirement.expected} · actual {requirement.status.replace('_', ' ')} ·
            {requirement.window_days === null
                ? 'no window'
                : `window ${requirement.window_days} days`}
            {#if requirement.scope_basis === 'house'}· scope set by the house{/if}
        </div>
        {#each requirement.entities as cell (cell.entity_id)}
            <section>
                <h3>
                    <span class="mono">{cell.entity_id}</span>
                    <span class="dim">{cell.tier}</span>
                    <Badge tone="neutral" label={cell.status.replace('_', ' ')} />
                </h3>
                <div class="dim">
                    {cell.streak_days} stored day{cell.streak_days === 1 ? '' : 's'} in a row
                    {#if cell.days_until_operating !== null}
                        · {cell.days_until_operating} to go until operating
                    {/if}
                </div>
                {#if cell.reason}<div class="dim">{cell.reason}</div>{/if}
                {#if cell.evidence && cell.evidence.length > 0}
                    <table class="data">
                        <thead>
                            <tr>
                                <th>Record</th><th>Source</th><th>Date</th><th>Outcome</th>
                                <th>Counts for</th>
                            </tr>
                        </thead>
                        <tbody>
                            {#each cell.evidence as e (e.evidence_id)}
                                <tr>
                                    <td>
                                        <span class="mono">{e.evidence_id}</span>
                                        {#if e.experiment}<div class="dim">{e.experiment}</div>{/if}
                                    </td>
                                    <td
                                        >{e.source}{e.kind ? ` · ${e.kind}` : ''}{e.exercise_type
                                            ? ` · ${e.exercise_type}`
                                            : ''}</td
                                    >
                                    <td class="mono">{day(e.at_ns)} ({e.age_days} d)</td>
                                    <td>{e.outcome}</td>
                                    <td class="mono">{pct(e.v)}</td>
                                </tr>
                            {/each}
                        </tbody>
                    </table>
                {:else}
                    <div class="state">No record on file for this function.</div>
                {/if}
            </section>
        {/each}
        {#if doc.disclaimer}<div class="dim foot">{doc.disclaimer}</div>{/if}
    {/if}
</div>

<style>
    .drawer {
        position: fixed;
        top: 0;
        right: 0;
        bottom: 0;
        width: min(620px, 100vw);
        overflow-y: auto;
        background: var(--bg-raised);
        border-left: var(--border-width-sm) solid var(--border-strong);
        padding: var(--space-4) var(--space-4-5);
        z-index: var(--z-overlay);
        box-shadow: -8px 0 24px rgb(0 0 0 / 25%);
    }
    .top {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: var(--space-3);
        margin-bottom: var(--space-2);
    }
    .top button {
        background: var(--bg-hover);
        color: var(--text);
        border: var(--border-width-sm) solid var(--border-strong);
        border-radius: var(--radius-md);
        padding: 3px var(--space-2-5);
        font: inherit;
        font-size: var(--font-size-xs);
        cursor: pointer;
    }
    .title {
        font-weight: var(--font-weight-strong);
    }
    section {
        margin-top: var(--space-3-5);
    }
    h3 {
        margin: 0 0 var(--space-0-5);
        font-size: var(--font-size-md);
        display: flex;
        gap: var(--space-2);
        align-items: baseline;
    }
    .dim {
        color: var(--text-faint);
        font-size: var(--font-size-2xs);
    }
    .foot {
        margin-top: var(--space-3-5);
    }
</style>

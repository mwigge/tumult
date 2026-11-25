<script module lang="ts">
    import type { DryRunScope } from '#lib/types.js';

    /** Fault actions naming at least one target — the "affects N targets" count. */
    export function targetedActions(scope: DryRunScope): number {
        return scope.actions.filter((a) => Object.keys(a.targets).length > 0).length;
    }
</script>

<script lang="ts">
    let { scope }: { scope: DryRunScope } = $props();

    /** One "key: value" target pair for display; non-strings render as JSON. */
    function targetPairs(targets: Record<string, unknown>): string {
        return Object.entries(targets)
            .map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
            .join(', ');
    }

    const affected = $derived(targetedActions(scope));
</script>

<div class="scope">
    {#if scope.blast_radius}
        <div class="note">{scope.blast_radius}</div>
    {/if}
    {#if scope.actions.length === 0}
        <div class="hint">No fault actions — this definition only measures.</div>
    {:else}
        <ul>
            {#each scope.actions as a, i (i)}
                <li>
                    <span class="mono">{a.step}</span>
                    <span class="meta mono">{a.provider} {a.action}</span>
                    {#if Object.keys(a.targets).length > 0}
                        <span class="targets mono">→ {targetPairs(a.targets)}</span>
                    {/if}
                </li>
            {/each}
        </ul>
        <div class="hint">
            Affects {affected} target{affected === 1 ? '' : 's'} across {scope.actions.length}
            fault action{scope.actions.length === 1 ? '' : 's'}{scope.max_concurrent_faults !== null
                ? ` · at most ${scope.max_concurrent_faults} concurrent`
                : ''}.
        </div>
    {/if}
    {#if scope.guards.length > 0}
        <h3>Guards — halt when breached</h3>
        <ul>
            {#each scope.guards as g, i (i)}
                <li>
                    <span class="mono">{g.name}</span>
                    <span class="meta">
                        probe {g.probe} · {g.min_breaches} breach{g.min_breaches === 1 ? '' : 'es'}
                    </span>
                </li>
            {/each}
        </ul>
    {/if}
</div>

<style>
    .note {
        border-left: var(--border-width-md) solid var(--warn);
        padding: var(--space-0-5) var(--space-2-5);
        color: var(--text-dim);
        font-size: var(--font-size-md);
        margin-bottom: var(--space-2);
    }
    .scope ul {
        margin: var(--space-1) 0;
        padding-left: var(--space-5-5);
    }
    .scope li {
        padding: var(--space-0-5) 0;
        font-size: var(--font-size-md);
    }
    .scope li .meta {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
        margin-left: var(--space-2);
    }
    .scope li .targets {
        color: var(--warn);
        font-size: var(--font-size-xs);
        margin-left: var(--space-2);
    }
    .hint {
        color: var(--text-dim);
        font-size: var(--font-size-sm);
        margin-top: var(--space-1-5);
    }
    h3 {
        font-size: var(--font-size-xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-md);
        color: var(--text-dim);
        margin: var(--space-3) 0 var(--space-1);
    }
</style>

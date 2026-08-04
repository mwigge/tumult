<script module lang="ts">
  import type { DryRunScope } from '$lib/types';

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
    border-left: 2px solid var(--warn);
    padding: 2px 10px;
    color: var(--text-dim);
    font-size: 13px;
    margin-bottom: 8px;
  }
  .scope ul {
    margin: 4px 0;
    padding-left: 22px;
  }
  .scope li {
    padding: 2px 0;
    font-size: 13px;
  }
  .scope li .meta {
    color: var(--text-faint);
    font-size: 12px;
    margin-left: 8px;
  }
  .scope li .targets {
    color: var(--warn);
    font-size: 12px;
    margin-left: 8px;
  }
  .hint {
    color: var(--text-dim);
    font-size: 12.5px;
    margin-top: 6px;
  }
  h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    margin: 12px 0 4px;
  }
</style>

<script lang="ts">
  // Scrollable audit trail for a run: timestamp, event, actor and detail per
  // entry, oldest first as returned by the API.
  import { fmtTs } from '$lib/api';
  import type { RunAuditEntry } from '$lib/types';

  let { audit }: { audit: RunAuditEntry[] } = $props();
</script>

<div class="panel">
  <h2>Audit trail ({audit.length})</h2>
  {#if audit.length === 0}
    <div class="state">No audit events yet.</div>
  {:else}
    <div class="audit">
      {#each audit as entry, i (i)}
        <div class="entry">
          <span class="mono ts">{fmtTs(entry.at_ns)}</span>
          <span class="mono event">{entry.event}</span>
          <span class="actor">{entry.actor ?? 'system'}</span>
          <span class="detail">{entry.detail ?? ''}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .audit {
    max-height: 420px;
    overflow-y: auto;
  }
  .entry {
    display: flex;
    gap: 10px;
    padding: 3px 0;
    font-size: 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
    align-items: baseline;
  }
  .ts {
    color: var(--text-faint);
    flex: 0 0 128px;
  }
  .event {
    flex: 0 0 150px;
  }
  .actor {
    color: var(--text-dim);
    flex: 0 0 110px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .detail {
    color: var(--text-dim);
    min-width: 0;
  }
</style>

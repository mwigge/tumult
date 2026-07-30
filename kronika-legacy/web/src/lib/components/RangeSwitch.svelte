<script lang="ts">
  // 24h / 7d / 14d segmented control.
  let { value, onchange }: { value: string; onchange: (range: string) => void } = $props();
  const RANGES = ['24h', '7d', '14d'];
</script>

<div class="seg" role="group" aria-label="time range">
  {#each RANGES as r (r)}
    <button class:active={value === r} onclick={() => onchange(r)}>{r}</button>
  {/each}
</div>

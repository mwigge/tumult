<script lang="ts">
  // Dependency-free SVG sparkline for KPI cards.
  import type { SparkPoint } from '$lib/types';

  let {
    points,
    color = 'var(--accent)',
    width = 132,
    height = 34
  }: { points: SparkPoint[]; color?: string; width?: number; height?: number } = $props();

  const geom = $derived.by(() => {
    if (points.length < 2) return null;
    const vs = points.map((p) => p.v);
    const min = Math.min(...vs);
    const max = Math.max(...vs);
    const span = max - min || 1;
    const step = width / (points.length - 1);
    const coords = points.map(
      (p, i) => `${(i * step).toFixed(1)},${(height - 3 - ((p.v - min) / span) * (height - 8)).toFixed(1)}`
    );
    return { line: coords.join(' '), area: `0,${height} ${coords.join(' ')} ${width},${height}` };
  });
</script>

{#if geom}
  <svg {width} {height} aria-hidden="true">
    <polygon points={geom.area} fill={color} opacity="0.12" />
    <polyline
      points={geom.line}
      fill="none"
      stroke={color}
      stroke-width="1.5"
      stroke-linejoin="round"
      stroke-linecap="round"
    />
  </svg>
{:else}
  <div class="empty" style="width: {width}px; height: {height}px;">—</div>
{/if}

<style>
  .empty {
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
  }
</style>

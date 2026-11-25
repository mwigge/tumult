<script lang="ts">
    // The filters of the DR page: one target, one environment and a period.
    // A target or an environment the URL names stays a choice when the
    // register does not hold it, so the page shows what the URL asks for.
    import { PERIOD_DAYS } from '#lib/dr.js';
    import type { DrFilters } from '#lib/dr.js';

    let {
        filters,
        systems,
        environments,
        onchange,
    }: {
        filters: DrFilters;
        systems: string[];
        environments: string[];
        onchange: (key: keyof DrFilters, value: string) => void;
    } = $props();

    const PERIOD_LABEL: Record<string, string> = {
        '90d': '90 days',
        '1y': '1 year',
        '3y': '3 years',
    };
    const choices = (values: string[], chosen: string) =>
        chosen && !values.includes(chosen) ? [...values, chosen].sort() : values;
</script>

<div class="filters">
    <select
        aria-label="Target"
        value={filters.target}
        onchange={(e) => onchange('target', e.currentTarget.value)}
    >
        <option value="">all targets</option>
        {#each choices(systems, filters.target) as system (system)}
            <option value={system}>{system}</option>
        {/each}
    </select>
    <select
        aria-label="Environment"
        value={filters.environment}
        onchange={(e) => onchange('environment', e.currentTarget.value)}
    >
        <option value="">all environments</option>
        {#each choices(environments, filters.environment) as environment (environment)}
            <option value={environment}>{environment}</option>
        {/each}
    </select>
    <div class="seg" role="group" aria-label="Period">
        {#each Object.keys(PERIOD_DAYS) as period (period)}
            <button
                class:active={filters.period === period}
                aria-pressed={filters.period === period}
                onclick={() => onchange('period', period)}>{PERIOD_LABEL[period]}</button
            >
        {/each}
    </div>
</div>

<style>
    .filters {
        display: flex;
        gap: var(--space-2);
        flex-wrap: wrap;
        align-items: center;
    }
</style>

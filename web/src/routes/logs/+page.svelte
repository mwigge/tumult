<script lang="ts">
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { onMount } from 'svelte';
    import { api, fmtAgo, fmtTs } from '#lib/api.js';
    import type { LogEntry, LogVolume } from '#lib/types.js';
    import Badge from '#lib/components/Badge.svelte';
    import RangeSwitch from '#lib/components/RangeSwitch.svelte';
    import EChart from '#lib/components/EChart.svelte';
    import { chartTheme, chartTokens, followTheme } from '#lib/echarts.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { bucketAxis } from '#lib/ranges.js';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';
    import { onRow } from '#lib/rows.js';

    const SEVERITIES = ['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'];

    // The chart is drawn in the colours of the theme in force.
    let theme = $state(chartTheme());
    onMount(() => followTheme((next) => (theme = next)));
    const tokens = $derived(chartTokens(theme));
    const sevColors: Record<string, string> = $derived({
        ERROR: tokens.fail,
        FATAL: tokens.fail,
        WARN: tokens.warn,
        WARNING: tokens.warn,
        INFO: tokens.ok,
        DEBUG: tokens.text,
        TRACE: tokens.text,
    });

    const filters = $derived({
        range: page.url.searchParams.get('range') ?? '24h',
        severity: page.url.searchParams.get('severity') ?? '',
        service: page.url.searchParams.get('service') ?? '',
        q: page.url.searchParams.get('q') ?? '',
        attr: page.url.searchParams.get('attr') ?? '',
        attr_not: page.url.searchParams.get('attr_not') ?? '',
    });

    let rows: LogEntry[] | null = $state(null);
    let volume: LogVolume | null = $state(null);
    let error: string | null = $state(null);
    let expanded: number | null = $state(null);

    /** Open the row `i` under it, or close it when it is open. */
    function toggle(i: number) {
        expanded = expanded === i ? null : i;
    }

    $effect(() => {
        // Touch every filter so the effect re-runs on any change; debounce for
        // the free-text inputs.
        const params = { ...filters };
        let cancelled = false;
        rows = null;
        error = null;
        const t = setTimeout(() => {
            api.logs(params)
                .then((r) => !cancelled && (rows = r.logs))
                .catch((e) => !cancelled && (error = String(e)));
            api.logsVolume({ ...params, interval: params.range === '24h' ? '1h' : '1d' })
                .then((v) => !cancelled && (volume = v))
                .catch(() => {});
        }, 200);
        return () => {
            cancelled = true;
            clearTimeout(t);
        };
    });

    function setFilter(key: string, value: string) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        if (value) params.set(key, value);
        else params.delete(key);
        goto(`?${params}`, { replace: true, reset: false });
    }

    // Click-to-filter (facet ⊕/⊖) from the detail attribute table: the
    // attr/attr_not URL params map to exact k=v predicates on the API.
    function filterFor(e: MouseEvent, k: string, v: string) {
        e.stopPropagation();
        setFilter('attr', `${k}=${v}`);
    }
    function filterOut(e: MouseEvent, k: string, v: string) {
        e.stopPropagation();
        setFilter('attr_not', `${k}=${v}`);
    }

    // Pivot the (bucket, severity) rows into one stacked bar series per
    // severity, aligned across the full bucket set so gaps render as zero.
    const volumeOption: EChartsCoreOption = $derived.by(() => {
        if (!volume || volume.rows.length === 0) return {};
        const buckets = [...new Set(volume.rows.map((r) => r.ts))].sort((a, b) => a - b);
        const ord = (s: string) => {
            const i = SEVERITIES.indexOf(s.toUpperCase());
            return i === -1 ? SEVERITIES.length : i;
        };
        const sevs = [...new Set(volume.rows.map((r) => r.severity))].sort(
            (a, b) => ord(a) - ord(b),
        );

        const byKey = new Map(volume.rows.map((r) => [`${r.ts}|${r.severity}`, r.count]));
        return {
            grid: { left: 48, right: 16, top: 28, bottom: 24 },
            tooltip: { trigger: 'axis' },
            legend: { top: 0 },
            xAxis: {
                type: 'time',
                // Bars would otherwise widen the axis by half a bucket,
                // past now.
                ...bucketAxis(
                    filters.range,
                    volume.bucket_s,
                    Date.now(),
                    buckets.map((ts) => ts * 1000),
                ),
                containShape: false,
            },
            yAxis: {
                type: 'value',
                minInterval: 1,
            },
            series: sevs.map((sev) => ({
                name: sev,
                type: 'bar',
                stack: 'volume',
                barMaxWidth: 40,
                itemStyle: { color: sevColors[sev.toUpperCase()] ?? tokens.accent },
                data: buckets.map((b) => [b * 1000, byKey.get(`${b}|${sev}`) ?? 0]),
            })),
        };
    });
</script>

<PageHeader
    title="Logs"
    sub={rows ? `${rows.length} row${rows.length === 1 ? '' : 's'}` : 'newest first'}
>
    <div class="controls">
        <input
            type="search"
            aria-label="Search body"
            placeholder="search body…"
            value={filters.q}
            oninput={(e) => setFilter('q', e.currentTarget.value)}
        />
        <select
            aria-label="Severity"
            value={filters.severity}
            onchange={(e) => setFilter('severity', e.currentTarget.value)}
        >
            <option value="">all severities</option>
            {#each SEVERITIES as s (s)}
                <option value={s}>{s}</option>
            {/each}
        </select>
        <input
            type="search"
            aria-label="Service"
            placeholder="service…"
            value={filters.service}
            oninput={(e) => setFilter('service', e.currentTarget.value)}
        />
        <RangeSwitch value={filters.range} onchange={(r) => setFilter('range', r)} />
    </div>
</PageHeader>

{#if filters.attr || filters.attr_not}
    <div class="facet-chips">
        {#if filters.attr}
            <button class="chip" title="remove filter" onclick={() => setFilter('attr', '')}>
                ⊕ {filters.attr} <span class="x">×</span>
            </button>
        {/if}
        {#if filters.attr_not}
            <button class="chip" title="remove filter" onclick={() => setFilter('attr_not', '')}>
                ⊖ {filters.attr_not} <span class="x">×</span>
            </button>
        {/if}
    </div>
{/if}

<Panel>
    {#if volume && volume.rows.length > 0}
        <EChart id="log-volume" option={volumeOption} height={180} label="Log volume by severity" />
    {:else if rows}
        <State kind="empty">No log volume in this window.</State>
    {:else}
        <State kind="loading" height={180} />
    {/if}
</Panel>

<Panel>
    {#if error}
        <State kind="error">Failed to load logs: {error}</State>
    {:else if !rows}
        <State kind="loading" height={240} />
    {:else if rows.length === 0}
        <State kind="empty">No logs match these filters.</State>
    {:else}
        <table class="data">
            <thead>
                <tr>
                    <th>Time</th><th>Severity</th><th>Service</th><th>Body</th>
                </tr>
            </thead>
            <tbody>
                {#each rows as row, i (row.ts_ns + row.body)}
                    <tr
                        class="clickable"
                        class:expanded={expanded === i}
                        onclick={onRow(() => toggle(i))}
                    >
                        <td class="mono" title={fmtTs(row.ts_ns)}>
                            <button
                                class="row-button"
                                aria-expanded={expanded === i}
                                onclick={() => toggle(i)}>{fmtAgo(row.ts_ns)}</button
                            >
                        </td>
                        <td>
                            <Badge
                                kind="severity"
                                value={row.severity_text}
                                label={row.severity_text ?? '—'}
                            />
                        </td>
                        <td class="mono">{row.service_name ?? '—'}</td>
                        <td class="body-cell">{row.body}</td>
                    </tr>
                    {#if expanded === i}
                        <tr class="detail-row">
                            <td colspan="4">
                                <div class="detail">
                                    <div class="links">
                                        {#if row.experiment_id}
                                            <a href="/experiments/{row.experiment_id}"
                                                >experiment {row.experiment_id}</a
                                            >
                                        {/if}
                                        {#if row.trace_id}
                                            <a href="/traces/{row.trace_id}">trace {row.trace_id}</a
                                            >
                                        {/if}
                                        {#if !row.experiment_id && !row.trace_id}
                                            <span class="sub">no experiment or trace linkage</span>
                                        {/if}
                                    </div>
                                    {#if Object.keys(row.log_attrs).length > 0}
                                        <table class="attrs">
                                            <tbody>
                                                {#each Object.entries(row.log_attrs) as [k, v] (k)}
                                                    <tr class="attr-row">
                                                        <td class="mono key">{k}</td>
                                                        <td class="mono">{v}</td>
                                                        <td class="facets">
                                                            <button
                                                                title="filter for {k}={v}"
                                                                onclick={(e) => filterFor(e, k, v)}
                                                                >⊕</button
                                                            >
                                                            <button
                                                                title="filter out {k}={v}"
                                                                onclick={(e) => filterOut(e, k, v)}
                                                                >⊖</button
                                                            >
                                                        </td>
                                                    </tr>
                                                {/each}
                                            </tbody>
                                        </table>
                                    {/if}
                                </div>
                            </td>
                        </tr>
                    {/if}
                {/each}
            </tbody>
        </table>
    {/if}
</Panel>

<style>
    .controls {
        margin-left: auto;
        display: flex;
        gap: var(--space-2);
        flex-wrap: wrap;
    }
    .body-cell {
        max-width: 640px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    tr.expanded td {
        border-bottom: none;
    }
    /* An open log row has no background of its own. */
    .detail-row td {
        padding-top: 0;
        background: none;
    }
    .detail {
        padding: var(--space-1) 0 var(--space-2);
    }
    .links {
        display: flex;
        gap: var(--space-4);
        margin-bottom: var(--space-2);
        font-size: var(--font-size-md);
    }
    table.attrs td {
        padding: var(--space-0-5) var(--space-2) var(--space-0-5) 0;
        border: none;
        font-size: var(--font-size-xs);
    }
    table.attrs .key {
        color: var(--text-dim);
    }
    .facets {
        visibility: hidden;
        white-space: nowrap;
    }
    .attr-row:hover .facets {
        visibility: visible;
    }
    .facets button {
        background: transparent;
        border: var(--border-width-sm) solid var(--border);
        border-radius: var(--radius-sm);
        color: var(--text-dim);
        cursor: pointer;
        font-size: var(--font-size-3xs);
        padding: 0 var(--space-1);
        margin-left: var(--space-1);
    }
    .facets button:hover {
        color: var(--accent);
        border-color: var(--accent);
    }
    .facet-chips {
        display: flex;
        gap: var(--space-2);
        margin: -6px 0 var(--space-2-5);
    }
</style>

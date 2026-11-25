<script lang="ts">
    // Disaster recovery: the DR test register of each target, the RTO and
    // RPO of each target over the period, and the DR tests of the period,
    // automated and manual together. The filters live in the URL; the table
    // pages with the cursor of the API.
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { onMount } from 'svelte';
    import { api } from '#lib/api.js';
    import {
        CHART_LIMIT,
        PAGE_SIZE,
        filterOptions,
        filtersOf,
        localDay,
        recoveryGroups,
        registerSummary,
        testsSummary,
        viewQuery,
    } from '#lib/dr.js';
    import type { DrFilters } from '#lib/dr.js';
    import type { DrQuery, DrView } from '#lib/types.js';
    import DrEmpty from '#lib/components/DrEmpty.svelte';
    import Filters from '#lib/components/DrFilters.svelte';
    import DrRecovery from '#lib/components/DrRecovery.svelte';
    import DrRegister from '#lib/components/DrRegister.svelte';
    import DrTestTable from '#lib/components/DrTestTable.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import State from '#lib/components/State.svelte';

    const filters = $derived(filtersOf(page.url.searchParams));
    const filtered = $derived(filters.target !== '' || filters.environment !== '');

    // The choices of the filters: every target of the register, read once.
    let options = $state<{ systems: string[]; environments: string[] }>({
        systems: [],
        environments: [],
    });
    // The register, the summary and the tests the charts draw.
    let view = $state<DrView | null>(null);
    // The page of the table, and the cursors of the pages after the first.
    let shown = $state<DrView | null>(null);
    let trail = $state<string[]>([]);
    let query: DrQuery = {};
    let error = $state<string | null>(null);
    let busy = $state(false);
    // A newer load makes the answers of an older one stale.
    let generation = 0;

    const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));

    onMount(() => {
        api.dr({ limit: 1 })
            .then((whole) => (options = filterOptions(whole.targets)))
            .catch((e) => (error = reason(e)));
    });

    $effect(() => {
        const q = viewQuery(filters, Date.now());
        const mine = ++generation;
        query = q;
        trail = [];
        view = null;
        shown = null;
        error = null;
        Promise.all([api.dr({ ...q, limit: CHART_LIMIT }), api.dr({ ...q, limit: PAGE_SIZE })])
            .then(([whole, first]) => {
                if (mine !== generation) return;
                view = whole;
                shown = first;
            })
            .catch((e) => {
                if (mine === generation) error = reason(e);
            });
    });

    async function turn(cursor: string | undefined, next: string[]) {
        const mine = generation;
        busy = true;
        try {
            const answer = await api.dr({ ...query, limit: PAGE_SIZE, cursor });
            if (mine !== generation) return;
            shown = answer;
            trail = next;
        } catch (e) {
            if (mine === generation) error = reason(e);
        } finally {
            busy = false;
        }
    }

    function nextPage() {
        const cursor = shown?.next_cursor;
        if (cursor) turn(cursor, [...trail, cursor]);
    }

    function previousPage() {
        const back = trail.slice(0, -1);
        turn(back.at(-1), back);
    }

    function setFilter(key: keyof DrFilters, value: string) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        if (value) params.set(key, value);
        else params.delete(key);
        goto(`?${params}`, { replace: true, reset: false });
    }
</script>

<PageHeader
    title="Disaster recovery"
    sub="The DR tests of each target, automated and manual, and when the next is due"
>
    <div class="controls">
        <Filters
            {filters}
            systems={options.systems}
            environments={options.environments}
            onchange={setFilter}
        />
    </div>
</PageHeader>

{#if error}
    <State kind="error">Failed to load the DR view: {error}</State>
{:else if !view || !shown}
    <State kind="loading" height={320} />
{:else if view.targets.length === 0 && view.total === 0 && !filtered}
    <DrEmpty />
{:else if view.targets.length === 0 && view.total === 0}
    <div class="panel">
        <State kind="empty" action={{ href: '/dr', label: 'Show every target' }}>
            No target and no DR test match these filters.
        </State>
    </div>
{:else}
    <div class="sections">
        <section class="panel" aria-labelledby="dr-register">
            <h2 id="dr-register">Due register</h2>
            <p class="summary" data-testid="dr-register-summary">
                {registerSummary(view.summary.states)}
            </p>
            <DrRegister targets={view.targets} />
        </section>
        <section class="panel" aria-labelledby="dr-recovery">
            <h2 id="dr-recovery">RTO and RPO over time</h2>
            <DrRecovery
                groups={recoveryGroups(view.tests, view.targets)}
                period={view}
                truncated={view.truncated}
            />
        </section>
        <section class="panel" aria-labelledby="dr-tests">
            <h2 id="dr-tests">DR tests in the period</h2>
            <p class="summary" data-testid="dr-tests-summary">
                {testsSummary(view.summary.tests)}, {localDay(view.from_ns)} to {localDay(
                    view.to_ns,
                )}
            </p>
            <DrTestTable
                tests={shown.tests}
                total={shown.total}
                page={trail.length + 1}
                pageSize={PAGE_SIZE}
                canPrev={trail.length > 0}
                canNext={shown.next_cursor !== null}
                {busy}
                onprev={previousPage}
                onnext={nextPage}
            />
        </section>
    </div>
{/if}

<style>
    .controls {
        margin-left: auto;
    }
    .sections {
        display: grid;
        gap: var(--space-3-5);
    }
    .panel {
        overflow-x: auto;
    }
    .summary {
        color: var(--text-dim);
        font-size: var(--font-size-sm);
        margin: -4px 0 var(--space-2-5);
    }
</style>

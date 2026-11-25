<script lang="ts">
    // The DR test register: per target, where it stands, its last DR test,
    // when the next one is due and why, the plan that covers it, and the RTO
    // and RPO it is held to against what its last test measured.
    import {
        PLANS_HREF,
        lastTestHref,
        localDay,
        rpoAlarm,
        rpoText,
        rtoAlarm,
        rtoText,
        stateLabel,
        verdictLabel,
    } from '#lib/dr.js';
    import type { DrTarget } from '#lib/types.js';
    import Badge from './Badge.svelte';
    import DrObjectiveCell from './DrObjectiveCell.svelte';

    let { targets }: { targets: DrTarget[] } = $props();

    const SOURCE: Record<DrTarget['interval_source'], string> = {
        plan: 'from its plan',
        function: 'from its function',
        default: 'by default',
    };
    const key = (target: DrTarget) => `${target.system}/${target.environment ?? '-'}`;
</script>

<table class="data">
    <caption class="sr-only">
        Due register: the last DR test of each target and when the next one is due
    </caption>
    <thead>
        <tr>
            <th scope="col">Target</th>
            <th scope="col">State</th>
            <th scope="col">Last test</th>
            <th scope="col">Next due</th>
            <th scope="col">Interval</th>
            <th scope="col">Plan</th>
            <th scope="col">RTO</th>
            <th scope="col">RPO</th>
        </tr>
    </thead>
    <tbody>
        {#each targets as target (key(target))}
            {@const last = target.last_test}
            <tr data-target={key(target)}>
                <th scope="row">
                    {target.system}
                    {#if target.environment}<span class="dim">· {target.environment}</span>{/if}
                </th>
                <td class="nowrap">
                    <Badge kind="dr" value={target.state} label={stateLabel(target.state)} />
                </td>
                <td class="last">
                    {#if last}
                        <a class="mono" href={lastTestHref(last)}>{localDay(last.executed_at_ns)}</a
                        >
                        <Badge tone="neutral" label={last.origin} />
                        <Badge
                            kind="verdict"
                            value={last.verdict}
                            label={verdictLabel(last.verdict)}
                        />
                    {:else}
                        <span class="dim">no DR test yet</span>
                    {/if}
                </td>
                <td class="mono nowrap">
                    {target.next_due_at_ns === null ? 'test now' : localDay(target.next_due_at_ns)}
                </td>
                <td class="nowrap">
                    <span class="line">every {target.interval_days} days</span>
                    <span class="line dim">{SOURCE[target.interval_source]}</span>
                </td>
                <td class="nowrap">
                    {#if target.plan}
                        <a href={PLANS_HREF}>due {localDay(target.plan.due_at_ns)}</a>
                    {:else}
                        <span class="dim">no plan</span>
                    {/if}
                </td>
                <td>
                    <DrObjectiveCell
                        measured={rtoText(target)}
                        target={target.rto_target_s}
                        alarm={rtoAlarm(target, target.rto_target_s)}
                    />
                </td>
                <td>
                    <DrObjectiveCell
                        measured={rpoText(target)}
                        target={target.rpo_target_s}
                        alarm={rpoAlarm(target, target.rpo_target_s)}
                    />
                </td>
            </tr>
        {/each}
    </tbody>
</table>

<style>
    /* A row header reads as a cell, not as a column header. */
    th[scope='row'] {
        color: var(--text);
        font-size: var(--font-size-md);
        font-weight: var(--font-weight-strong);
        text-transform: none;
        letter-spacing: normal;
        border-bottom: var(--border-width-sm) solid var(--border);
        padding: 7px var(--space-2-5);
    }
    tr:last-child th[scope='row'] {
        border-bottom: none;
    }
    /* Not the size of the global .dim: the size around it. */
    .dim {
        color: var(--text-faint);
        font-weight: 400;
        font-size: inherit;
    }
    .line {
        display: block;
    }
    .last,
    .nowrap,
    th[scope='row'] {
        white-space: nowrap;
    }
</style>

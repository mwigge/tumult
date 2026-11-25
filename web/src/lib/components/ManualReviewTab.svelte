<script lang="ts">
    // Review tabs: the verification queue (the submitted records that wait
    // for their sign-offs) and the list of all manual records. A row opens to
    // the record: its DR detail, sign-offs, audit trail and verify, and its
    // evidence. The API enforces who may sign off; the page shows its reason.
    import { api, fmtTs } from '#lib/api.js';
    import { errorText, ofTarget, type TargetFilter } from '#lib/manual.js';
    import type { ManualDetail, ManualExperiment, ManualPlan } from '#lib/types.js';
    import Badge from './Badge.svelte';
    import ManualRecordDetail from './ManualRecordDetail.svelte';
    import Panel from './Panel.svelte';
    import State from './State.svelte';
    import { onRow } from '#lib/rows.js';

    let {
        tab,
        actor,
        canReview,
        canEdit,
        filter = { target: '', environment: '' },
        record = null,
        onedit,
    }: {
        tab: string;
        actor: string;
        canReview: boolean;
        canEdit: boolean;
        /** The target whose records the lists show; empty fields show all. */
        filter?: TargetFilter;
        /** The id of the record that is open when the list is shown. */
        record?: string | null;
        onedit: (id: string) => Promise<void>;
    } = $props();

    let statusFilter = $state('');
    let records: ManualExperiment[] | null = $state(null);
    let recordsError: string | null = $state(null);
    let openId: string | null = $state(null);
    let detail: ManualDetail | null = $state(null);
    let plans: ManualPlan[] = $state([]);

    const queue = $derived(tab === 'queue');
    const shown = $derived(records ? ofTarget<ManualExperiment>(records, filter) : null);
    const ofOne = $derived(filter.target !== '' || filter.environment !== '');

    function loadRecords() {
        records = null;
        recordsError = null;
        api.manualList(queue ? 'submitted' : statusFilter)
            .then((r) => (records = r.records))
            .catch((e) => (recordsError = errorText(e)));
    }

    /** The record `id`, and its row updated to where it stands now. */
    function loadDetail(id: string) {
        api.manualDetail(id)
            .then((d) => {
                if (openId === id) detail = d;
                records = records?.map((r) => (r.id === id ? d.experiment : r)) ?? records;
            })
            .catch((e) => (recordsError = errorText(e)));
    }

    function toggle(id: string) {
        detail = null;
        if (openId === id) {
            openId = null;
            return;
        }
        openId = id;
        loadDetail(id);
    }

    /** After a sign-off, an upload or a link: the record again. It stays in
        the list until the list is loaded again, so its answer stays in view. */
    function changed() {
        if (openId) loadDetail(openId);
    }

    $effect(() => {
        // Reload when the tab or the status filter changes.
        void statusFilter;
        void tab;
        openId = null;
        loadRecords();
    });
    // After the effect above, so the record of the URL stays open.
    $effect(() => {
        if (!record) return;
        openId = record;
        loadDetail(record);
    });
    $effect(() => {
        api.manualPlans()
            .then((r) => (plans = r.plans ?? []))
            .catch(() => (plans = []));
    });

    const planName = (id: string | null | undefined) =>
        plans.find((p) => p.id === id)?.name ?? id ?? '';
</script>

<Panel>
    <div class="records-head">
        <h2>{queue ? 'Submitted records awaiting sign-off' : 'All manual records'}</h2>
        {#if !queue}
            <select aria-label="Status" bind:value={statusFilter}>
                <option value="">all statuses</option>
                {#each ['draft', 'submitted', 'verified', 'rejected'] as s (s)}
                    <option value={s}>{s}</option>
                {/each}
            </select>
        {/if}
    </div>
    {#if recordsError}
        <State kind="error">{recordsError}</State>
    {/if}
    {#if !shown}
        {#if !recordsError}<State kind="loading" height={160} />{/if}
    {:else if shown.length === 0}
        <State kind="empty">
            {queue ? 'Nothing awaiting sign-off' : 'No manual records'}{ofOne
                ? ' of this target'
                : ''}.
        </State>
    {:else}
        <table class="data">
            <thead>
                <tr
                    ><th>Status</th><th>Experiment</th><th>Type</th><th>Outcome</th><th>Executed</th
                    ><th>Entered</th><th>Verifier</th></tr
                >
            </thead>
            <tbody>
                {#each shown as rec (rec.id)}
                    <tr
                        class="clickable"
                        data-record={rec.id}
                        onclick={onRow(() => toggle(rec.id))}
                    >
                        <td><Badge kind="review" value={rec.status} /></td>
                        <td>
                            <button
                                class="row-button"
                                aria-expanded={openId === rec.id}
                                onclick={() => toggle(rec.id)}>{rec.experiment_name}</button
                            >
                        </td>
                        <td><Badge tone="neutral" label={rec.exercise_type} /></td>
                        <td><Badge kind="status" value={rec.outcome_status} /></td>
                        <td class="mono">{fmtTs(rec.executed_at_ns)}</td>
                        <td>
                            {rec.entered_by}
                            {#if actor.trim() === rec.entered_by && rec.status === 'submitted'}
                                <span
                                    class="warn-text"
                                    title="a reviewer must differ from the enterer">⚠ you</span
                                >
                            {/if}
                        </td>
                        <td
                            >{rec.reviewed_by ?? '—'}
                            {#if rec.status === 'draft' && canEdit}
                                <button
                                    class="btn"
                                    onclick={(event) => {
                                        event.stopPropagation();
                                        void onedit(rec.id);
                                    }}>Edit draft</button
                                >
                            {/if}
                        </td>
                    </tr>
                    {#if openId === rec.id}
                        <tr class="detail-row">
                            <td colspan="7">
                                {#if !detail}
                                    <State kind="loading" height={60} />
                                {:else}
                                    <ManualRecordDetail
                                        {detail}
                                        planName={planName(detail.experiment.plan_id)}
                                        {actor}
                                        {canReview}
                                        canWrite={canEdit}
                                        onchange={changed}
                                    />
                                {/if}
                            </td>
                        </tr>
                    {/if}
                {/each}
            </tbody>
        </table>
    {/if}
</Panel>

<style>
    .warn-text {
        color: var(--warn);
        font-size: var(--font-size-2xs);
        margin-left: var(--space-1-5);
    }
    .records-head {
        display: flex;
        justify-content: space-between;
        align-items: center;
    }
</style>

<script lang="ts">
    // Plans tab: the planned manual exercises with their due dates, overdue
    // ones marked, and the form to plan one or change a planned one. A row
    // opens to the audit trail of the plan and its audit verify, or to the
    // cancel of a planned plan.
    import { api } from '#lib/api.js';
    import {
        emptyPlanForm,
        errorText,
        ofTarget,
        planPatch,
        planRequest,
        planToForm,
        type TargetFilter,
    } from '#lib/manual.js';
    import type { ManualPlan, ManualPlanDetail } from '#lib/types.js';
    import ManualAuditList from './ManualAuditList.svelte';
    import ManualPlanForm from './ManualPlanForm.svelte';
    import ManualPlanTable from './ManualPlanTable.svelte';
    import ManualVerify from './ManualVerify.svelte';

    let {
        actor,
        canWrite,
        filter = { target: '', environment: '' },
    }: {
        actor: string;
        canWrite: boolean;
        /** The target whose plans the table shows; empty fields show all. */
        filter?: TargetFilter;
    } = $props();

    let plans: ManualPlan[] | null = $state(null);
    let loadError: string | null = $state(null);
    let nowNs = $state(Date.now() * 1e6);
    let form = $state(emptyPlanForm());
    let editingId: string | null = $state(null);
    let busy = $state(false);
    let message: { ok: boolean; text: string } | null = $state(null);
    let openId: string | null = $state(null);
    let cancelling = $state(false);
    let reason = $state('');
    let opened: ManualPlanDetail | null = $state(null);
    let cancelMessage: string | null = $state(null);

    function load() {
        loadError = null;
        nowNs = Date.now() * 1e6;
        api.manualPlans()
            .then((r) => (plans = r.plans ?? []))
            .catch((e) => (loadError = errorText(e)));
    }
    $effect(() => load());

    async function save() {
        busy = true;
        message = null;
        try {
            if (editingId) {
                await api.manualPlanEdit(editingId, planPatch(form, actor));
                message = { ok: true, text: `plan ${editingId} changed` };
            } else {
                const created = await api.manualPlanCreate(planRequest(form, actor));
                message = { ok: true, text: `plan ${created.id} saved` };
            }
            editingId = null;
            form = emptyPlanForm();
            load();
        } catch (e) {
            message = { ok: false, text: errorText(e) };
        } finally {
            busy = false;
        }
    }

    function edit(plan: ManualPlan) {
        editingId = plan.id;
        form = planToForm(plan);
        message = null;
    }

    function discard() {
        editingId = null;
        form = emptyPlanForm();
        message = null;
    }

    function open(plan: ManualPlan, cancel = false) {
        if (openId === plan.id && cancelling === cancel) {
            openId = null;
            return;
        }
        openId = plan.id;
        cancelling = cancel;
        cancelMessage = null;
        reason = '';
        opened = null;
        if (cancel) return;
        api.manualPlanDetail(plan.id)
            .then((d) => (opened = d))
            .catch((e) => (cancelMessage = errorText(e)));
    }

    async function cancelPlan(plan: ManualPlan) {
        busy = true;
        cancelMessage = null;
        try {
            await api.manualPlanCancel(plan.id, actor.trim(), reason.trim() || undefined);
            openId = null;
            load();
        } catch (e) {
            cancelMessage = errorText(e);
        } finally {
            busy = false;
        }
    }
</script>

<div class="plans">
    <div class="panel">
        <h2>Plans</h2>
        {#if loadError}
            <div class="state error">{loadError}</div>
        {:else if !plans}
            <div class="skeleton" style="height: 120px"></div>
        {:else}
            <ManualPlanTable
                plans={ofTarget(plans, filter)}
                {nowNs}
                {canWrite}
                {openId}
                onopen={(p) => open(p)}
                onedit={edit}
                oncancel={(p) => open(p, true)}
            >
                {#snippet detail(plan)}
                    {#if cancelling}
                        <div class="cancel">
                            <label class="grow"
                                >Reason to cancel <input type="text" bind:value={reason} /></label
                            >
                            <button
                                class="btn danger"
                                disabled={busy}
                                onclick={() => cancelPlan(plan)}>Cancel this plan</button
                            >
                            <button class="btn" onclick={() => (openId = null)}>Keep it</button>
                        </div>
                    {:else if opened}
                        <div class="plan-detail">
                            <div>
                                <p class="dim">
                                    Function {plan.function_id ?? '—'} · scope {plan.scope ?? '—'}
                                    · created by {plan.created_by}
                                </p>
                                <ManualAuditList rows={opened.audit} />
                            </div>
                            <ManualVerify run={() => api.manualPlanAuditVerify(plan.id)} />
                        </div>
                    {:else if !cancelMessage}
                        <div class="skeleton" style="height: 40px"></div>
                    {/if}
                    {#if cancelMessage}<div class="state error">{cancelMessage}</div>{/if}
                {/snippet}
            </ManualPlanTable>
        {/if}
    </div>
    <div class="panel form-panel">
        <h2>{editingId ? 'Edit plan' : 'Plan an exercise'}</h2>
        {#if canWrite}
            <ManualPlanForm
                bind:form
                editing={!!editingId}
                {busy}
                {message}
                onsave={save}
                ondiscard={discard}
            />
        {:else}
            <p>Your role can read plans. An operator can plan an exercise.</p>
        {/if}
    </div>
</div>

<style>
    .plans {
        display: flex;
        flex-direction: column;
        gap: var(--space-3-5);
    }
    .form-panel {
        max-width: 760px;
    }
    .cancel,
    .plan-detail {
        display: flex;
        gap: var(--space-3);
        align-items: flex-end;
        padding: var(--space-1-5) var(--space-0-5);
    }
    .plan-detail {
        justify-content: space-between;
        align-items: flex-start;
    }
    .cancel label {
        display: flex;
        flex-direction: column;
        gap: var(--space-1);
        font-size: var(--font-size-xs);
        color: var(--text-dim);
    }
    .grow {
        flex: 1;
    }
    .dim {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
    }
</style>

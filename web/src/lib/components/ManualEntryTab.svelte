<script lang="ts">
    // Entry tab: the record of a DR exercise, or of any hand-executed test:
    // what was tested and the plan it fulfils, the recovery against its
    // targets, the people and the tester, the TLPT block of a TLPT, the
    // steps, the actions with owners and severities, and the attestation. The form state is owned by the page (bindable) so a
    // half-filled draft survives switching to another tab and back. Once the
    // draft is saved, evidence can be attached to it before it is submitted.
    import { api } from '#lib/api.js';
    import {
        checkForm,
        emptyForm,
        errorText,
        EXERCISE_TYPES,
        formToRecord,
        OUTCOMES,
        SEVERITIES,
        type ManualForm,
    } from '#lib/manual.js';
    import type { ManualDetail, ManualPlan } from '#lib/types.js';
    import ManualActionsEditor from './ManualActionsEditor.svelte';
    import ManualAttachments from './ManualAttachments.svelte';
    import ManualParticipantsEditor from './ManualParticipantsEditor.svelte';
    import ManualPlanPicker from './ManualPlanPicker.svelte';
    import ManualRecoveryFields from './ManualRecoveryFields.svelte';
    import ManualStepsEditor from './ManualStepsEditor.svelte';
    import ManualTesterFields from './ManualTesterFields.svelte';
    import ManualTlptFields from './ManualTlptFields.svelte';

    let {
        actor,
        allowedEnvironments = [],
        editingId = $bindable<string | null>(null),
        form = $bindable(),
        entryMsg = $bindable(),
        busy = $bindable(),
    }: {
        actor: string;
        allowedEnvironments?: string[];
        editingId?: string | null;
        form: ManualForm;
        entryMsg: { ok: boolean; text: string } | null;
        busy: boolean;
    } = $props();

    let plans: ManualPlan[] = $state([]);
    let draft: ManualDetail | null = $state(null);

    $effect(() => {
        api.manualPlans('planned')
            .then((r) => (plans = r.plans ?? []))
            .catch((e) => (entryMsg = { ok: false, text: errorText(e) }));
    });

    function loadDraft(id: string) {
        api.manualDetail(id)
            .then((d) => {
                if (editingId === id) draft = d;
            })
            .catch((e) => (entryMsg = { ok: false, text: errorText(e) }));
    }
    $effect(() => {
        const id = editingId;
        draft = null;
        if (id) loadDraft(id);
    });

    async function save(andSubmit: boolean) {
        entryMsg = null;
        const refused = checkForm(form, actor, allowedEnvironments);
        if (refused) {
            entryMsg = { ok: false, text: refused };
            return;
        }
        const rec = formToRecord(form, actor);
        busy = true;
        try {
            if (editingId) {
                await api.manualUpdate(editingId, rec);
            } else {
                const created = await api.manualCreate(rec);
                editingId = created.id;
            }
            const id = editingId;
            if (andSubmit) await api.manualSubmit(id, rec.entered_by);
            entryMsg = {
                ok: true,
                text: andSubmit
                    ? `record ${id} created and submitted for verification`
                    : `draft ${id} saved`,
            };
            if (andSubmit) {
                form = emptyForm();
                editingId = null;
            }
        } catch (e) {
            entryMsg = { ok: false, text: errorText(e) };
        } finally {
            busy = false;
        }
    }
</script>

<div class="entry">
    <div class="grid cols-2">
        <div class="panel">
            <h2>Exercise</h2>
            <div class="form">
                <label
                    >Experiment name
                    <input
                        type="text"
                        bind:value={form.experiment_name}
                        placeholder="payments-db failover to the standby"
                    />
                </label>
                <div class="row2">
                    <label
                        >Exercise type
                        <select bind:value={form.exercise_type}>
                            {#each EXERCISE_TYPES as t (t)}<option value={t}>{t}</option>{/each}
                        </select>
                    </label>
                    <label
                        >Executed at
                        <input type="datetime-local" bind:value={form.executed} />
                    </label>
                </div>
                <div class="row2">
                    <label
                        >Target system <input type="text" bind:value={form.target_system} /></label
                    >
                    <label
                        >Target environment <input
                            type="text"
                            bind:value={form.target_environment}
                        /></label
                    >
                </div>
                <ManualPlanPicker bind:form {plans} />
                <label
                    >Hypothesis
                    <textarea
                        rows="2"
                        bind:value={form.hypothesis}
                        placeholder="What steady state did you expect?"></textarea>
                </label>
                <label
                    >Method
                    <textarea
                        rows="3"
                        bind:value={form.method}
                        placeholder="How was the test executed?"></textarea>
                </label>
                <div class="row2">
                    <label
                        >Outcome
                        <select bind:value={form.outcome_status}>
                            {#each OUTCOMES as o (o)}<option value={o}>{o}</option>{/each}
                        </select>
                    </label>
                    <label
                        >Hypothesis held
                        <select bind:value={form.hypothesis_met}>
                            <option value="unknown">not recorded</option>
                            <option value="met">met</option>
                            <option value="not-met">not met</option>
                        </select>
                    </label>
                </div>
                <label
                    >Findings
                    <textarea rows="2" bind:value={form.findings}></textarea>
                </label>
                <label
                    >Findings severity
                    <select bind:value={form.findings_severity}>
                        <option value="">not set</option>
                        {#each SEVERITIES as s (s)}<option value={s}>{s}</option>{/each}
                    </select>
                </label>
                <div class="row2">
                    <label>Blast radius <input type="text" bind:value={form.blast_radius} /></label>
                    <label
                        >Duration (s) <input
                            type="number"
                            step="1"
                            bind:value={form.duration_s}
                        /></label
                    >
                </div>
                <label
                    >Framework refs (comma-sep) <input
                        type="text"
                        bind:value={form.framework_refs}
                        placeholder="dora.art24.6, ISO 27001 A.5.30"
                    /></label
                >
                <label
                    >Plan scenarios (comma-sep) <input
                        type="text"
                        bind:value={form.scenarios}
                        placeholder="premises_loss, power_outage"
                    /></label
                >
            </div>
        </div>
        <div class="stack">
            <div class="panel">
                <h2>Recovery</h2>
                <ManualRecoveryFields bind:form />
            </div>
            <div class="panel">
                <h2>Participants</h2>
                <ManualParticipantsEditor bind:rows={form.participants} />
            </div>
            <div class="panel">
                <h2>Tester</h2>
                <ManualTesterFields bind:form />
            </div>
            {#if form.exercise_type === 'tlpt'}
                <div class="panel">
                    <h2>TLPT</h2>
                    <ManualTlptFields bind:form />
                </div>
            {/if}
        </div>
    </div>
    <div class="panel">
        <h2>Steps</h2>
        <ManualStepsEditor bind:rows={form.steps} />
    </div>
    <div class="panel">
        <h2>{form.exercise_type === 'tlpt' ? 'Remediation plan' : 'Actions'}</h2>
        <ManualActionsEditor bind:rows={form.actions} />
    </div>
    <div class="grid cols-2">
        <div class="panel">
            <h2>Attestation</h2>
            <div class="form">
                <label
                    >Renewal due (optional)
                    <input type="date" bind:value={form.renewal} />
                </label>
                <label
                    >Attestation text
                    <textarea
                        rows="5"
                        bind:value={form.attestation}
                        placeholder="I attest this record reflects the exercise as executed, and the findings are complete to the best of my knowledge."
                    ></textarea>
                </label>
                <p class="note">
                    Submitting locks the record. Each sign-off needs a reviewer other than
                    <b>{actor.trim() || 'the person entering it'}</b>. Verified records score like
                    automated runs (passed 100 / partial 75 / failed 50; inconclusive is excluded);
                    drafts and submitted records count toward coverage as pending.
                </p>
                <div class="actions">
                    <button class="btn" disabled={busy} onclick={() => save(false)}
                        >Save draft</button
                    >
                    <button class="btn primary" disabled={busy} onclick={() => save(true)}
                        >Save &amp; submit</button
                    >
                </div>
                {#if entryMsg}
                    <div class="state" class:error={!entryMsg.ok} role="status">
                        {entryMsg.text}
                    </div>
                {/if}
            </div>
        </div>
        <div class="panel">
            <h2>Evidence</h2>
            {#if draft?.experiment}
                <ManualAttachments
                    experimentId={draft.experiment.id}
                    attachments={draft.attachments}
                    status={draft.experiment.status}
                    {actor}
                    canWrite={true}
                    onchange={() => editingId && loadDraft(editingId)}
                />
            {:else}
                <p class="note">Save the draft to attach files and links to it.</p>
            {/if}
        </div>
    </div>
</div>

<style>
    .entry,
    .stack {
        display: flex;
        flex-direction: column;
        gap: var(--space-3-5);
    }
</style>

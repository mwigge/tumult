<script lang="ts">
    // The form of a plan: a new one, or the planned plan being edited. The
    // tab owns the form and sends it; the API checks every value.
    import { EXERCISE_TYPES, type PlanForm } from '#lib/manual.js';

    let {
        form = $bindable(),
        editing,
        busy,
        message,
        onsave,
        ondiscard,
    }: {
        form: PlanForm;
        editing: boolean;
        busy: boolean;
        message: { ok: boolean; text: string } | null;
        onsave?: () => void;
        ondiscard?: () => void;
    } = $props();
</script>

<div class="form">
    <label>Plan name <input type="text" bind:value={form.name} /></label>
    <div class="row2">
        <label
            >Exercise type
            <select bind:value={form.exercise_type}>
                {#each EXERCISE_TYPES as t (t)}<option value={t}>{t}</option>{/each}
            </select>
        </label>
        <label>Owner <input type="text" bind:value={form.owner} /></label>
    </div>
    <div class="row2">
        <label>Target system <input type="text" bind:value={form.target_system} /></label>
        <label>Target environment <input type="text" bind:value={form.target_environment} /></label>
    </div>
    <div class="row2">
        <label
            >Function <input
                type="text"
                bind:value={form.function_id}
                placeholder="function id of the register"
            /></label
        >
        <label>Scope <input type="text" bind:value={form.scope} /></label>
    </div>
    <div class="row2">
        <label>Planned date <input type="date" bind:value={form.planned} /></label>
        <label>Due date <input type="date" bind:value={form.due} /></label>
    </div>
    <div class="row3">
        <label
            >Repeat every (days) <input
                type="number"
                min="1"
                step="1"
                bind:value={form.interval_days}
            /></label
        >
        <label
            >Planned RTO (s) <input
                type="number"
                min="0"
                step="any"
                bind:value={form.rto_target_s}
            /></label
        >
        <label
            >Planned RPO (s) <input
                type="number"
                min="0"
                step="any"
                bind:value={form.rpo_target_s}
            /></label
        >
    </div>
    <p class="note">
        A plan is due by the end of its due date. A DR plan with no interval repeats every 365 days.
        When a verified record fulfils the plan, the next one is planned.
    </p>
    <div class="actions">
        <button class="btn primary" disabled={busy} onclick={() => onsave?.()}
            >{editing ? 'Save changes' : 'Save plan'}</button
        >
        {#if editing}
            <button class="btn" disabled={busy} onclick={() => ondiscard?.()}>Stop editing</button>
        {/if}
    </div>
    {#if message}
        <div class="state" class:error={!message.ok} role="status">{message.text}</div>
    {/if}
</div>

<style>
    .row3 {
        display: grid;
        grid-template-columns: 1fr 1fr 1fr;
        gap: var(--space-2-5);
    }
</style>

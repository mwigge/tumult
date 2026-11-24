<script lang="ts">
    // The plan a record fulfils once it is verified: one of the planned plans
    // of the same exercise type and target. Picking it fills what the form
    // leaves blank.
    import { applyPlan, matchingPlans, localDay, type ManualForm } from '#lib/manual.js';
    import type { ManualPlan } from '#lib/types.js';

    let { form = $bindable(), plans }: { form: ManualForm; plans: ManualPlan[] } = $props();

    const fits = $derived(matchingPlans(plans, form));
    const kept = $derived(!!form.plan_id && !fits.some((p) => p.id === form.plan_id));

    function pick(id: string) {
        const plan = plans.find((p) => p.id === id);
        form = plan ? applyPlan(form, plan) : { ...form, plan_id: id };
    }
</script>

<label
    >Plan it fulfils
    <select value={form.plan_id} onchange={(e) => pick(e.currentTarget.value)}>
        <option value="">no plan</option>
        {#each fits as p (p.id)}
            <option value={p.id}>{p.name} · due {localDay(p.due_at_ns)}</option>
        {/each}
        {#if kept}<option value={form.plan_id}>{form.plan_id}</option>{/if}
    </select>
</label>

<script lang="ts">
    // The recovery of a DR exercise: the RTO and RPO planned and reached,
    // and whether the data was checked after it.
    import type { ManualForm } from '#lib/manual.js';

    let { form = $bindable() }: { form: ManualForm } = $props();
</script>

<div class="form">
    <div class="row2">
        <label
            >Planned RTO (s) <input
                type="number"
                min="0"
                step="any"
                bind:value={form.rto_target_s}
            /></label
        >
        <label
            >Actual recovery time (s) <input
                type="number"
                min="0"
                step="any"
                bind:value={form.recovery_time_s}
            /></label
        >
    </div>
    <div class="row2">
        <label
            >Planned RPO (s) <input
                type="number"
                min="0"
                step="any"
                bind:value={form.rpo_target_s}
            /></label
        >
        <label
            >Actual RPO (s) <input
                type="number"
                min="0"
                step="any"
                bind:value={form.rpo_measured_s}
            /></label
        >
    </div>
    <label
        >Data verified
        <select bind:value={form.data_verified}>
            <option value="unknown">not checked</option>
            <option value="yes">complete</option>
            <option value="no">loss or corruption found</option>
        </select>
    </label>
    <p class="note">
        The RPO is the seconds of writes from before the outage that were lost. A recovery at or
        under its planned value meets it.
    </p>
</div>

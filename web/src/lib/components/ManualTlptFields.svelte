<script lang="ts">
    // The TLPT block of a record of type tlpt: the attestation the TLPT
    // authority issued and its date, and the test summary report. The
    // remediation plan is the actions of the record.
    import type { ManualForm } from '#lib/manual.js';

    let { form = $bindable() }: { form: ManualForm } = $props();
</script>

<div class="form">
    <div class="row2">
        <label
            >Attestation reference <input
                type="text"
                placeholder="as the TLPT authority gave it"
                bind:value={form.tlpt_attestation_ref}
            /></label
        >
        <label>Attestation date <input type="date" bind:value={form.tlpt_attested} /></label>
    </div>
    <label
        >Test summary report reference <input
            type="text"
            bind:value={form.tlpt_summary_ref}
        /></label
    >
    <p class="note">
        The attestation is RTS 2025/1190 Art. 14, the test summary report Art. 12. The remediation
        plan (Art. 13) is the actions below: give each an owner, a due date, a severity and the run
        or record that retested it.
    </p>
</div>

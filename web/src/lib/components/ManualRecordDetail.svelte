<script lang="ts">
    // One record opened in a list: its DR detail, its sign-offs, its audit
    // trail and seal with the audit verify, its evidence and attestation.
    import { api } from '#lib/api.js';
    import type { ManualDetail } from '#lib/types.js';
    import ManualAttachments from './ManualAttachments.svelte';
    import ManualAuditList from './ManualAuditList.svelte';
    import ManualExerciseView from './ManualExerciseView.svelte';
    import ManualSignOff from './ManualSignOff.svelte';
    import ManualVerify from './ManualVerify.svelte';

    let {
        detail,
        planName = '',
        actor,
        canReview,
        canWrite,
        onchange,
    }: {
        detail: ManualDetail;
        planName?: string;
        actor: string;
        canReview: boolean;
        canWrite: boolean;
        onchange?: () => void;
    } = $props();

    const rec = $derived(detail.experiment);

    // A change made here (a sign-off, a file, a link) makes an earlier
    // verdict of the audit verify stale: it is cleared at once, not when
    // the record is read again.
    let changes = $state(0);
    function changed() {
        changes += 1;
        onchange?.();
    }
</script>

<ManualExerciseView {detail} {planName} />
<div class="detail">
    <div>
        <h3>Sign-off</h3>
        <ManualSignOff
            experimentId={rec.id}
            status={rec.status}
            signoffs={detail.signoffs ?? []}
            required={detail.signoffs_required ?? 1}
            needed={detail.signoffs_needed ?? 0}
            {actor}
            {canReview}
            onchange={changed}
        />
        <h3>Audit trail</h3>
        <ManualAuditList rows={detail.audit} />
        <div class="dim hash">content hash {rec.content_hash.slice(0, 16)}…</div>
        {#key changes}
            <ManualVerify run={() => api.manualAuditVerify(rec.id)} />
        {/key}
    </div>
    <div>
        <h3>Evidence</h3>
        <ManualAttachments
            experimentId={rec.id}
            attachments={detail.attachments}
            status={rec.status}
            {actor}
            {canWrite}
            onchange={changed}
        />
        {#if rec.attestation}
            <h3>Attestation</h3>
            <p class="dim">{rec.attestation}</p>
        {/if}
    </div>
</div>

<style>
    .detail {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: var(--space-4);
        padding: var(--space-2) var(--space-1);
    }
    @media (max-width: 900px) {
        .detail {
            grid-template-columns: 1fr;
        }
    }
    h3 {
        font-size: var(--font-size-xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-sm);
        color: var(--text-dim);
        margin: var(--space-2-5) 0 var(--space-1-5);
    }
    .hash {
        margin: var(--space-1-5) 0;
        font-family: var(--mono);
    }
</style>

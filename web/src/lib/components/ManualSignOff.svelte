<script lang="ts">
    // The sign-offs of a record and how many it still needs. An approval is
    // a sign-off; the last one the exercise type needs verifies the record.
    // The API refuses a signer who entered or changed the record, or signed
    // it before, and the page shows its reason.
    import { api, fmtTs } from '#lib/api.js';
    import { errorText, signoffSummary } from '#lib/manual.js';
    import type { ManualSignOffRow } from '#lib/types.js';

    let {
        experimentId,
        status,
        signoffs,
        required,
        needed,
        actor,
        canReview,
        onchange,
    }: {
        experimentId: string;
        status: string;
        signoffs: ManualSignOffRow[];
        required: number;
        needed: number;
        actor: string;
        canReview: boolean;
        onchange?: () => void;
    } = $props();

    let note = $state('');
    let busy = $state(false);
    let message: { ok: boolean; text: string } | null = $state(null);

    const approvals = $derived(signoffs.filter((s) => s.decision === 'approved').length);

    async function sign(approve: boolean) {
        busy = true;
        message = null;
        try {
            const r = approve
                ? await api.manualVerify(experimentId, actor.trim(), note.trim() || undefined)
                : await api.manualReject(experimentId, actor.trim(), note.trim());
            const after = r.signoffs_required - r.signoffs_needed;
            const summary = signoffSummary(r.status, after, r.signoffs_required, r.signoffs_needed);
            message = { ok: true, text: approve ? `Approved. ${summary}` : 'Rejected.' };
            note = '';
            onchange?.();
        } catch (e) {
            message = { ok: false, text: errorText(e) };
        } finally {
            busy = false;
        }
    }
</script>

<p class="summary" data-testid="signoff-summary">
    {signoffSummary(status, approvals, required, needed)}
</p>
{#if signoffs.length > 0}
    <ol class="signoffs">
        {#each signoffs as s (s.id)}
            <li>
                <span class="badge {s.decision === 'approved' ? 'ok' : 'fail'}">{s.decision}</span>
                by {s.signed_by} <span class="dim">· {fmtTs(s.signed_at_ns)}</span>
                {#if s.note}<span class="dim">: {s.note}</span>{/if}
            </li>
        {/each}
    </ol>
{/if}
{#if status === 'draft'}
    <p class="dim">A record is signed off once it is submitted.</p>
{:else if status === 'submitted' && !canReview}
    <p class="dim">An approver signs off a record.</p>
{:else if status === 'submitted'}
    <div class="sign">
        <label class="grow">Sign-off note <input type="text" bind:value={note} /></label>
        <button class="btn primary" disabled={busy} onclick={() => sign(true)}>Approve</button>
        <button class="btn danger" disabled={busy} onclick={() => sign(false)}>Reject</button>
    </div>
    <p class="dim">
        A reject needs a note. The person who entered, changed or attached evidence to the record
        cannot sign it off.
    </p>
{/if}
{#if message}
    <div class="state" class:error={!message.ok} role="status">{message.text}</div>
{/if}

<style>
    .summary {
        margin: 0 0 6px;
    }
    .signoffs {
        margin: 0 0 8px;
        padding-left: 18px;
    }
    .sign {
        display: flex;
        gap: 8px;
        align-items: flex-end;
    }
    .sign label {
        display: flex;
        flex-direction: column;
        gap: 4px;
        font-size: 12px;
        color: var(--text-dim);
    }
    .grow {
        flex: 1;
    }
    .dim {
        color: var(--text-faint);
        font-size: 12px;
    }
</style>

<script lang="ts">
    import { api } from '#lib/api.js';
    import type { ValidateToonResponse } from '#lib/types.js';
    import State from './State.svelte';

    let {
        toon = $bindable(),
        result = $bindable(),
    }: { toon: string; result: ValidateToonResponse | null } = $props();

    let validating = $state(false);
    let validateError = $state<string | null>(null);

    // Editing the TOON invalidates the last validation — the banner follows
    // the text, not the other way round.
    function onEdit(e: Event) {
        toon = (e.currentTarget as HTMLTextAreaElement).value;
        result = null;
        validateError = null;
    }

    async function validate() {
        if (validating || toon.trim() === '') return;
        validating = true;
        validateError = null;
        try {
            result = await api.validateToon(toon);
        } catch (e) {
            // Registration needs the operator role; a 403 lands here and is shown
            // as-is — the API is the gate, the UI stays out of the way.
            result = null;
            validateError = String(e);
        } finally {
            validating = false;
        }
    }
</script>

<textarea class="toon" aria-label="Experiment TOON" spellcheck="false" value={toon} oninput={onEdit}
></textarea>

<div class="actions">
    <button class="primary" onclick={validate} disabled={validating || toon.trim() === ''}>
        {validating ? 'Validating…' : 'Validate & register'}
    </button>
    <span class="hint"
        >Validation registers the definition (content-hash deduped) and requires the operator role.</span
    >
</div>

{#if validateError}
    <State kind="error">{validateError}</State>
{:else if result && !result.valid}
    <State kind="error">Invalid experiment: {result.error}</State>
{:else if result && result.valid}
    <div class="ok">
        Valid — registered as <span class="mono">{result.registry_id}</span>{result.registered
            ? ''
            : ' (identical TOON was already registered)'}.
    </div>
{/if}

<style>
    .toon {
        width: 100%;
        min-height: 22rem;
        font-family: var(--mono);
        font-size: var(--font-size-xs);
        line-height: var(--line-height-body);
        white-space: pre;
    }
    .actions {
        display: flex;
        align-items: center;
        gap: var(--space-3);
        margin: var(--space-3) 0;
    }
    .hint {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
        margin-top: 0;
    }
    .ok {
        color: var(--ok);
        font-size: var(--font-size-md);
        margin-bottom: var(--space-2);
    }
</style>

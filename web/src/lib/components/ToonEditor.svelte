<script lang="ts">
  import { api } from '$lib/api';
  import type { ValidateToonResponse } from '$lib/types';

  let {
    toon = $bindable(),
    result = $bindable()
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

<textarea class="toon" spellcheck="false" value={toon} oninput={onEdit}></textarea>

<div class="actions">
  <button class="primary" onclick={validate} disabled={validating || toon.trim() === ''}>
    {validating ? 'Validating…' : 'Validate & register'}
  </button>
  <span class="hint">Validation registers the definition (content-hash deduped) and requires the
    operator role.</span>
</div>

{#if validateError}
  <div class="state error">{validateError}</div>
{:else if result && !result.valid}
  <div class="state error">Invalid experiment: {result.error}</div>
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
    font-size: 12.5px;
    line-height: 1.5;
    white-space: pre;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 10px 0;
  }
  .hint {
    color: var(--text-dim);
    font-size: 12.5px;
  }
  .ok {
    color: var(--ok);
    font-size: 13px;
    margin-bottom: 6px;
  }
</style>

<script lang="ts" module>
  export const emptyForm = () => ({
    experiment_name: '',
    exercise_type: 'gameday',
    executed: '',
    hypothesis: '',
    method: '',
    outcome_status: 'passed',
    hypothesis_met: 'unknown',
    findings: '',
    action_items: '',
    target_system: '',
    target_environment: '',
    blast_radius: '',
    recovery_time_s: '',
    duration_s: '',
    framework_refs: '',
    renewal: '',
    attestation: ''
  });
  export type ManualForm = ReturnType<typeof emptyForm>;
</script>

<script lang="ts">
  // Entry tab: the hand-executed test record form plus the attestation panel.
  // The form state is owned by the page (bindable) so a half-filled draft
  // survives switching to another tab and back.
  import { api } from '$lib/api';
  import type { ManualRecordInput } from '$lib/types';

  let {
    actor,
    form = $bindable(),
    entryMsg = $bindable(),
    busy = $bindable()
  }: {
    actor: string;
    form: ManualForm;
    entryMsg: { ok: boolean; text: string } | null;
    busy: boolean;
  } = $props();

  function toNs(local: string): number | null {
    if (!local) return null;
    const ms = new Date(local).getTime();
    return Number.isNaN(ms) ? null : ms * 1_000_000;
  }

  function buildRecord(): ManualRecordInput | string {
    const executed_at_ns = toNs(form.executed);
    if (!executed_at_ns) return 'executed date/time is required';
    if (!form.experiment_name.trim()) return 'experiment name is required';
    if (!form.hypothesis.trim()) return 'hypothesis is required';
    if (!form.method.trim()) return 'method is required';
    if (!form.attestation.trim()) return 'attestation is required';
    if (!actor.trim()) return 'set the "acting as" name first';
    return {
      experiment_name: form.experiment_name.trim(),
      exercise_type: form.exercise_type,
      executed_at_ns,
      hypothesis: form.hypothesis.trim(),
      method: form.method.trim(),
      outcome_status: form.outcome_status,
      hypothesis_met:
        form.hypothesis_met === 'unknown' ? null : form.hypothesis_met === 'met',
      findings: form.findings.trim() || null,
      action_items: form.action_items
        .split('\n')
        .map((s) => s.trim())
        .filter(Boolean),
      target_system: form.target_system.trim() || null,
      target_environment: form.target_environment.trim() || null,
      blast_radius: form.blast_radius.trim() || null,
      recovery_time_s: form.recovery_time_s ? Number(form.recovery_time_s) : null,
      duration_s: form.duration_s ? Number(form.duration_s) : null,
      entered_by: actor.trim(),
      attestation: form.attestation.trim(),
      renewal_due_ns: form.renewal ? toNs(`${form.renewal}T00:00`) : null,
      framework_refs: form.framework_refs
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean)
    };
  }

  async function save(andSubmit: boolean) {
    const rec = buildRecord();
    entryMsg = null;
    if (typeof rec === 'string') {
      entryMsg = { ok: false, text: rec };
      return;
    }
    busy = true;
    try {
      const { id } = await api.manualCreate(rec);
      if (andSubmit) await api.manualSubmit(id, rec.entered_by);
      entryMsg = {
        ok: true,
        text: andSubmit
          ? `record ${id} created and submitted for verification`
          : `draft ${id} saved`
      };
      form = emptyForm();
    } catch (e) {
      entryMsg = { ok: false, text: String(e) };
    } finally {
      busy = false;
    }
  }
</script>

<div class="grid cols-2">
  <div class="panel">
    <h2>Test record</h2>
    <div class="form">
      <label>Experiment name
        <input type="text" bind:value={form.experiment_name} placeholder="cdn failover — edge PoP loss" />
      </label>
      <div class="row2">
        <label>Exercise type
          <select bind:value={form.exercise_type}>
            {#each ['gameday', 'tabletop', 'failover', 'pentest', 'drill', 'other'] as t (t)}
              <option value={t}>{t}</option>
            {/each}
          </select>
        </label>
        <label>Executed at
          <input type="datetime-local" bind:value={form.executed} />
        </label>
      </div>
      <label>Hypothesis
        <textarea rows="2" bind:value={form.hypothesis} placeholder="What steady state did you expect?"></textarea>
      </label>
      <label>Method
        <textarea rows="3" bind:value={form.method} placeholder="How was the test executed?"></textarea>
      </label>
      <div class="row2">
        <label>Outcome
          <select bind:value={form.outcome_status}>
            {#each ['passed', 'partial', 'failed', 'inconclusive'] as o (o)}
              <option value={o}>{o}</option>
            {/each}
          </select>
        </label>
        <label>Hypothesis
          <select bind:value={form.hypothesis_met}>
            <option value="unknown">not recorded</option>
            <option value="met">met</option>
            <option value="not-met">not met</option>
          </select>
        </label>
      </div>
      <label>Findings
        <textarea rows="2" bind:value={form.findings}></textarea>
      </label>
      <label>Action items (one per line)
        <textarea rows="2" bind:value={form.action_items}></textarea>
      </label>
      <div class="row2">
        <label>Target system <input type="text" bind:value={form.target_system} /></label>
        <label>Target environment <input type="text" bind:value={form.target_environment} /></label>
      </div>
      <div class="row2">
        <label>Blast radius <input type="text" bind:value={form.blast_radius} /></label>
        <label>Recovery time (s) <input type="number" step="0.1" bind:value={form.recovery_time_s} /></label>
      </div>
      <div class="row2">
        <label>Duration (s) <input type="number" step="1" bind:value={form.duration_s} /></label>
        <label>Framework refs (comma-sep) <input type="text" bind:value={form.framework_refs} placeholder="DORA Art. 24(7), ISO 27001 A.5.30" /></label>
      </div>
    </div>
  </div>
  <div class="panel">
    <h2>Attestation</h2>
    <div class="form">
      <label>Renewal due (optional)
        <input type="date" bind:value={form.renewal} />
      </label>
      <label>Attestation text
        <textarea rows="5" bind:value={form.attestation}
          placeholder="I attest this record reflects the exercise as executed, and the findings are complete to the best of my knowledge."></textarea>
      </label>
      <p class="note">
        Submitting locks the record. Verification requires a reviewer other than
        <b>{actor.trim() || 'the person entering it'}</b>. Verified records score like automated
        runs (passed 100 / partial 75 / failed 50; inconclusive is excluded); drafts and
        submitted records count toward coverage as pending.
      </p>
      <div class="actions">
        <button class="btn" disabled={busy} onclick={() => save(false)}>Save draft</button>
        <button class="btn primary" disabled={busy} onclick={() => save(true)}>Save &amp; submit</button>
      </div>
      {#if entryMsg}
        <div class="state" class:error={!entryMsg.ok}>{entryMsg.text}</div>
      {/if}
    </div>
  </div>
</div>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .form label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .row2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .note {
    color: var(--text-faint);
    font-size: 12px;
    line-height: 1.5;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .btn {
    background: var(--bg-hover);
    border: 1px solid var(--border-strong);
    color: var(--text);
    border-radius: 6px;
    padding: 6px 14px;
    cursor: pointer;
    font-size: 13px;
  }
  .btn:hover {
    border-color: var(--accent-dim);
  }
  .btn.primary {
    background: var(--accent-dim);
    border-color: var(--accent-dim);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>

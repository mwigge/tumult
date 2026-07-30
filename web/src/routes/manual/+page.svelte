<script lang="ts">
  // Manual evidence: enter hand-executed test records under attestation,
  // and verify/reject submitted records (reviewer ≠ enterer — enforced by
  // the API). There is no auth yet: the "acting as" name is a plain string.
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { api, fmtTs } from '$lib/api';
  import type { ManualDetail, ManualExperiment, ManualRecordInput } from '$lib/types';

  const tab = $derived($page.url.searchParams.get('tab') ?? 'entry');

  let actor = $state('');
  onMount(() => {
    actor = localStorage.getItem('kronika.actor') ?? '';
  });
  function setActor(v: string) {
    actor = v;
    localStorage.setItem('kronika.actor', v);
  }

  function setTab(t: string) {
    const params = new URLSearchParams($page.url.searchParams);
    params.set('tab', t);
    goto(`?${params}`, { replaceState: true, keepFocus: true, noScroll: true });
  }

  // ---------------------------------------------------------------- entry
  const emptyForm = () => ({
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
  let form = $state(emptyForm());
  let entryMsg: { ok: boolean; text: string } | null = $state(null);
  let busy = $state(false);

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

  // ---------------------------------------------------------------- queue
  let queue: ManualExperiment[] | null = $state(null);
  let queueError: string | null = $state(null);
  let notes: Record<string, string> = $state({});
  let queueMsg: { ok: boolean; text: string } | null = $state(null);

  function loadQueue() {
    queue = null;
    queueError = null;
    api
      .manualList('submitted')
      .then((r) => (queue = r.records))
      .catch((e) => (queueError = String(e)));
  }

  async function review(id: string, approve: boolean) {
    queueMsg = null;
    if (!actor.trim()) {
      queueMsg = { ok: false, text: 'set the "acting as" name first' };
      return;
    }
    try {
      if (approve) {
        await api.manualVerify(id, actor.trim(), notes[id]?.trim() || undefined);
        queueMsg = { ok: true, text: 'record verified' };
      } else {
        const note = notes[id]?.trim() ?? '';
        if (!note) {
          queueMsg = { ok: false, text: 'reject requires a note' };
          return;
        }
        await api.manualReject(id, actor.trim(), note);
        queueMsg = { ok: true, text: 'record rejected' };
      }
      loadQueue();
    } catch (e) {
      queueMsg = { ok: false, text: String(e) };
    }
  }

  // ---------------------------------------------------------------- records
  let statusFilter = $state('');
  let records: ManualExperiment[] | null = $state(null);
  let recordsError: string | null = $state(null);
  let openId: string | null = $state(null);
  let detail: ManualDetail | null = $state(null);

  function loadRecords() {
    records = null;
    recordsError = null;
    api
      .manualList(statusFilter)
      .then((r) => (records = r.records))
      .catch((e) => (recordsError = String(e)));
  }

  async function toggle(id: string) {
    if (openId === id) {
      openId = null;
      detail = null;
      return;
    }
    openId = id;
    detail = null;
    try {
      detail = await api.manualDetail(id);
    } catch (e) {
      detail = null;
      recordsError = String(e);
    }
  }

  $effect(() => {
    const t = tab;
    if (t === 'queue') loadQueue();
    if (t === 'records') loadRecords();
  });
  $effect(() => {
    // Reload records when the status filter changes while on the tab.
    void statusFilter;
    if (tab === 'records') loadRecords();
  });

  const lifecycleClass = (s: string) =>
    s === 'verified' ? 'ok' : s === 'submitted' ? 'warn' : s === 'rejected' ? 'fail' : 'neutral';
  const outcomeClass = (s: string) =>
    s === 'passed' ? 'ok' : s === 'partial' ? 'warn' : s === 'failed' ? 'fail' : 'neutral';
</script>

<div class="page-head">
  <h1>Manual evidence</h1>
  <span class="sub">hand-executed tests, entered under attestation</span>
  <div class="controls">
    <label class="actor">
      acting as
      <input
        type="text"
        placeholder="your name"
        value={actor}
        oninput={(e) => setActor(e.currentTarget.value)}
      />
    </label>
    <div class="seg" role="group" aria-label="manual sections">
      {#each ['entry', 'queue', 'records'] as t (t)}
        <button class:active={tab === t} onclick={() => setTab(t)}>
          {t === 'entry' ? 'New entry' : t === 'queue' ? 'Verification queue' : 'All records'}
        </button>
      {/each}
    </div>
  </div>
</div>

{#if tab === 'entry'}
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
{:else if tab === 'queue'}
  <div class="panel">
    <h2>Submitted records awaiting verification</h2>
    {#if queueMsg}
      <div class="state" class:error={!queueMsg.ok}>{queueMsg.text}</div>
    {/if}
    {#if queueError}
      <div class="state error">{queueError}</div>
    {:else if !queue}
      <div class="skeleton" style="height: 160px"></div>
    {:else if queue.length === 0}
      <div class="state">Nothing awaiting verification.</div>
    {:else}
      <table class="data">
        <thead>
          <tr><th>Experiment</th><th>Outcome</th><th>Executed</th><th>Entered by</th><th>Note</th><th></th></tr>
        </thead>
        <tbody>
          {#each queue as rec (rec.id)}
            <tr>
              <td>{rec.experiment_name}</td>
              <td><span class="badge {outcomeClass(rec.outcome_status)}">{rec.outcome_status}</span></td>
              <td class="mono">{fmtTs(rec.executed_at_ns)}</td>
              <td>
                {rec.entered_by}
                {#if actor.trim() === rec.entered_by}
                  <span class="warn-text" title="a reviewer must differ from the enterer">⚠ you</span>
                {/if}
              </td>
              <td><input type="text" placeholder="review note…" bind:value={notes[rec.id]} /></td>
              <td class="actions">
                <button class="btn" onclick={() => review(rec.id, true)}>Verify</button>
                <button class="btn danger" onclick={() => review(rec.id, false)}>Reject</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
{:else}
  <div class="panel">
    <div class="records-head">
      <h2>All manual records</h2>
      <select bind:value={statusFilter}>
        <option value="">all statuses</option>
        {#each ['draft', 'submitted', 'verified', 'rejected'] as s (s)}
          <option value={s}>{s}</option>
        {/each}
      </select>
    </div>
    {#if recordsError}
      <div class="state error">{recordsError}</div>
    {:else if !records}
      <div class="skeleton" style="height: 160px"></div>
    {:else if records.length === 0}
      <div class="state">No manual records.</div>
    {:else}
      <table class="data">
        <thead>
          <tr><th>Status</th><th>Experiment</th><th>Outcome</th><th>Executed</th><th>Entered</th><th>Verifier</th></tr>
        </thead>
        <tbody>
          {#each records as rec (rec.id)}
            <tr class="clickable" onclick={() => toggle(rec.id)}>
              <td><span class="badge {lifecycleClass(rec.status)}">{rec.status}</span></td>
              <td>{rec.experiment_name}</td>
              <td><span class="badge {outcomeClass(rec.outcome_status)}">{rec.outcome_status}</span></td>
              <td class="mono">{fmtTs(rec.executed_at_ns)}</td>
              <td>{rec.entered_by}</td>
              <td>{rec.reviewed_by ?? '—'}</td>
            </tr>
            {#if openId === rec.id}
              <tr class="detail-row">
                <td colspan="6">
                  {#if !detail}
                    <div class="skeleton" style="height: 60px"></div>
                  {:else}
                    <div class="detail">
                      <div>
                        <h3>Audit trail</h3>
                        <ol class="audit">
                          {#each detail.audit as a (a.id)}
                            <li>
                              <span class="mono">{a.action}</span> by {a.changed_by}
                              <span class="dim">— {fmtTs(a.changed_at_ns)}</span>
                            </li>
                          {/each}
                        </ol>
                        <div class="dim hash">content hash {detail.experiment.content_hash.slice(0, 16)}…</div>
                      </div>
                      <div>
                        <h3>Attachments</h3>
                        {#if detail.attachments.length === 0}
                          <div class="dim">none</div>
                        {:else}
                          <ul class="attachments">
                            {#each detail.attachments as at (at.id)}
                              <li>
                                <span class="badge neutral">{at.kind}</span>
                                <a href={at.uri} target="_blank" rel="noreferrer">{at.label ?? at.uri}</a>
                              </li>
                            {/each}
                          </ul>
                        {/if}
                        {#if detail.experiment.attestation}
                          <h3>Attestation</h3>
                          <p class="dim">{detail.experiment.attestation}</p>
                        {/if}
                      </div>
                    </div>
                  {/if}
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
{/if}

<style>
  .controls {
    margin-left: auto;
    display: flex;
    gap: 12px;
    align-items: center;
  }
  .actor {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
    font-size: 12px;
  }
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
  .btn.danger {
    border-color: var(--fail);
    color: var(--fail);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .warn-text {
    color: var(--warn);
    font-size: 11.5px;
    margin-left: 6px;
  }
  .records-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .detail-row td {
    background: var(--bg-raised);
  }
  .detail {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
    padding: 8px 4px;
  }
  .detail h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-dim);
    margin: 0 0 6px;
  }
  .audit {
    margin: 0;
    padding-left: 18px;
  }
  .audit li {
    margin-bottom: 3px;
  }
  .dim {
    color: var(--text-faint);
    font-size: 12px;
  }
  .hash {
    margin-top: 6px;
    font-family: var(--mono);
  }
  .attachments {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .attachments a {
    color: var(--accent);
  }
</style>

<script lang="ts">
  // Review tabs: the verification queue (verify/reject submitted records —
  // reviewer ≠ enterer, enforced by the API) and the browsable list of all
  // manual records with per-record audit trail, attachments and attestation.
  import { api, fmtTs } from '$lib/api';
  import type { ManualDetail, ManualExperiment } from '$lib/types';

  let {
    tab,
    actor,
    notes = $bindable()
  }: {
    tab: string;
    actor: string;
    notes: Record<string, string>;
  } = $props();

  // ---------------------------------------------------------------- queue
  let queue: ManualExperiment[] | null = $state(null);
  let queueError: string | null = $state(null);
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

{#if tab === 'queue'}
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

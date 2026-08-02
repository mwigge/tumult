<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtAgo, fmtIn, fmtInterval, fmtTs } from '$lib/api';
  import type { MeResponse, RegistryEntry, Schedule } from '$lib/types';

  const PRESETS: { label: string; seconds: number }[] = [
    { label: 'every 15m', seconds: 900 },
    { label: 'hourly', seconds: 3600 },
    { label: 'every 6h', seconds: 21600 },
    { label: 'daily', seconds: 86400 },
    { label: 'weekly', seconds: 604800 }
  ];

  let me = $state<MeResponse | null>(null);
  let schedules = $state<Schedule[] | null>(null);
  let error = $state<string | null>(null);

  // Create form.
  let registry = $state<RegistryEntry[] | null>(null);
  let newName = $state('');
  let newRegistryId = $state('');
  let newInterval = $state(3600);
  let newEnv = $state('dev');
  let newVars = $state('');
  let createError = $state<string | null>(null);
  let creating = $state(false);

  // Row actions: enable/disable is safe; delete uses the per-row arm/confirm
  // pattern (first click arms, second within 5s deletes).
  let deleteArmedId = $state<string | null>(null);
  let rowError = $state<string | null>(null);
  let busy = $state(false);

  // Mutations are operator-level; viewing is open to every role. Open local
  // mode (no users) allows everything.
  const canMutate = $derived(
    me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false
  );

  onMount(() => {
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
    refresh();
    api
      .registry()
      .then((r) => (registry = r.definitions))
      .catch(() => (registry = []));
  });

  async function refresh() {
    try {
      const r = await api.schedules();
      schedules = r.schedules;
      error = null;
    } catch (e) {
      if (!schedules) error = String(e);
    }
  }

  /** Optional vars as a JSON object string; empty means none. */
  function parseVars(): Record<string, string> | null {
    const raw = newVars.trim();
    if (raw === '') return {};
    try {
      const parsed: unknown = JSON.parse(raw);
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return null;
      return Object.fromEntries(Object.entries(parsed).map(([k, v]) => [k, String(v)]));
    } catch {
      return null;
    }
  }

  async function create() {
    if (creating) return;
    const vars = parseVars();
    if (vars === null) {
      createError = 'vars must be a JSON object, e.g. {"host": "db-1"}';
      return;
    }
    creating = true;
    createError = null;
    try {
      await api.createSchedule({
        name: newName.trim(),
        registry_id: newRegistryId,
        interval_s: newInterval,
        vars,
        env: newEnv.trim() || 'dev'
      });
      newName = '';
      newVars = '';
      await refresh();
    } catch (e) {
      createError = String(e);
    } finally {
      creating = false;
    }
  }

  async function act(fn: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    rowError = null;
    try {
      await fn();
      await refresh();
    } catch (e) {
      rowError = String(e);
    } finally {
      busy = false;
    }
  }

  const toggleEnabled = (s: Schedule) => act(() => api.setScheduleEnabled(s.id, !s.enabled));

  function confirmDelete(s: Schedule) {
    if (deleteArmedId !== s.id) {
      deleteArmedId = s.id;
      setTimeout(() => {
        if (deleteArmedId === s.id) deleteArmedId = null;
      }, 5000);
      return;
    }
    deleteArmedId = null;
    act(() => api.deleteSchedule(s.id));
  }
</script>

<div class="page-head">
  <h1>Schedules</h1>
  <span class="sub">
    {schedules
      ? `${schedules.length} recurring run${schedules.length === 1 ? '' : 's'}`
      : 'interval-based recurring runs'}
  </span>
</div>

<div class="panel" style="margin-bottom: 14px">
  {#if error}
    <div class="state error">Failed to load schedules: {error}</div>
  {:else if !schedules}
    <div class="skeleton" style="height: 160px"></div>
  {:else if schedules.length === 0}
    <div class="state">No schedules yet — create one below to run a definition on an interval.</div>
  {:else}
    <table class="data">
      <thead>
        <tr>
          <th>Name</th><th>Definition</th><th>Interval</th><th>Next fire</th>
          <th>Last run</th><th>Env</th><th>Status</th><th>Created by</th>
          {#if canMutate}<th></th>{/if}
        </tr>
      </thead>
      <tbody>
        {#each schedules as s (s.id)}
          <tr>
            <td>{s.name}</td>
            <td>{s.definition_name ?? s.registry_id}</td>
            <td class="mono">{fmtInterval(s.interval_s)}</td>
            <td title={fmtTs(s.next_run_at_ns)}>{s.enabled ? fmtIn(s.next_run_at_ns) : '—'}</td>
            <td>
              {#if s.last_run_id}
                <a href="/runs/{s.last_run_id}">{s.last_run_at_ns ? fmtAgo(s.last_run_at_ns) : 'view'}</a>
              {:else}
                <span style="color: var(--text-faint)">never</span>
              {/if}
            </td>
            <td>{s.env}</td>
            <td><span class="badge {s.enabled ? 'ok' : 'neutral'}">{s.enabled ? 'enabled' : 'disabled'}</span></td>
            <td>{s.created_by ?? '—'}</td>
            {#if canMutate}
              <td class="actions">
                <button class="btn" onclick={() => toggleEnabled(s)} disabled={busy}>
                  {s.enabled ? 'Disable' : 'Enable'}
                </button>
                <button class="btn danger" onclick={() => confirmDelete(s)} disabled={busy}>
                  {deleteArmedId === s.id ? 'Confirm?' : 'Delete'}
                </button>
              </td>
            {/if}
          </tr>
        {/each}
      </tbody>
    </table>
    {#if rowError}<div class="state error" style="margin-top: 8px">{rowError}</div>{/if}
  {/if}
</div>

{#if canMutate}
  <div class="panel">
    <h2>New schedule</h2>
    {#if registry && registry.length === 0}
      <div class="state">
        No validated definitions yet — validate an experiment TOON first (Runs → New run).
      </div>
    {:else}
      <form
        class="create"
        onsubmit={(e) => {
          e.preventDefault();
          create();
        }}
      >
        <label>
          Name
          <input type="text" bind:value={newName} placeholder="hourly cpu stress" autocomplete="off" required />
        </label>
        <label>
          Definition
          <select bind:value={newRegistryId} required>
            <option value="" disabled>pick a registered definition…</option>
            {#each registry ?? [] as d (d.id)}
              <option value={d.id}>{d.name}</option>
            {/each}
          </select>
        </label>
        <label>
          Interval
          <select bind:value={newInterval}>
            {#each PRESETS as p (p.seconds)}
              <option value={p.seconds}>{p.label}</option>
            {/each}
          </select>
        </label>
        <label>
          Environment
          <input type="text" bind:value={newEnv} placeholder="dev" />
        </label>
        <label>
          Vars (optional JSON object)
          <input type="text" bind:value={newVars} placeholder={'{"host": "db-1"}'} />
        </label>
        <div>
          <button
            class="primary"
            type="submit"
            disabled={creating || !newName.trim() || !newRegistryId}
          >
            {creating ? 'Creating…' : 'Create schedule'}
          </button>
        </div>
      </form>
      <div class="hint">
        Scheduled runs go through the normal run path — production-classified environments
        still park for approval.
      </div>
      {#if createError}<div class="state error" style="margin-top: 10px">{createError}</div>{/if}
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
    white-space: nowrap;
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
  .create {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 12px 18px;
    align-items: end;
  }
  .create label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .hint {
    color: var(--text-faint);
    font-size: 12px;
    margin-top: 10px;
  }
</style>

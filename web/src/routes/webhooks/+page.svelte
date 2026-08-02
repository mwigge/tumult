<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtAgo, fmtTs } from '$lib/api';
  import type { MeResponse, Webhook } from '$lib/types';

  let me = $state<MeResponse | null>(null);
  let hooks = $state<Webhook[] | null>(null);
  let error = $state<string | null>(null);

  // Create form.
  let newName = $state('');
  let newUrl = $state('');
  let newEvents = $state('');
  let createError = $state<string | null>(null);
  // Shown once after creating — the HMAC secret is never retrievable again.
  let oneTime = $state<{ name: string; secret: string } | null>(null);
  let creating = $state(false);

  // Row actions.
  let deleteArmedId = $state<string | null>(null);
  let rowError = $state<string | null>(null);
  let busy = $state(false);

  // Admin-only (the server enforces 403 regardless); open local mode lets
  // anyone in.
  const isAdmin = $derived(me ? !me.auth_required || me.role === 'admin' : false);

  onMount(() => {
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
  });

  $effect(() => {
    if (!isAdmin || hooks !== null) return;
    refresh();
  });

  async function refresh() {
    try {
      const r = await api.webhooks();
      hooks = r.webhooks;
      error = null;
    } catch (e) {
      if (!hooks) error = String(e);
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

  const toggleEnabled = (w: Webhook) => act(() => api.setWebhookEnabled(w.id, !w.enabled));

  function confirmDelete(w: Webhook) {
    if (deleteArmedId !== w.id) {
      deleteArmedId = w.id;
      setTimeout(() => {
        if (deleteArmedId === w.id) deleteArmedId = null;
      }, 5000);
      return;
    }
    deleteArmedId = null;
    act(() => api.deleteWebhook(w.id));
  }

  async function create() {
    if (creating) return;
    creating = true;
    createError = null;
    oneTime = null;
    try {
      const events = newEvents
        .split(',')
        .map((s) => s.trim())
        .filter((s) => s !== '');
      const r = await api.createWebhook({ name: newName.trim(), url: newUrl.trim(), events });
      oneTime = { name: r.name, secret: r.secret };
      newName = '';
      newUrl = '';
      newEvents = '';
      await refresh();
    } catch (e) {
      createError = String(e);
    } finally {
      creating = false;
    }
  }
</script>

<div class="page-head">
  <h1>Webhooks</h1>
  <span class="sub">
    {hooks ? `${hooks.length} sink${hooks.length === 1 ? '' : 's'}` : 'signed run-event notifications'}
  </span>
</div>

{#if me && !isAdmin}
  <div class="panel">
    <div class="state">Webhook management requires the admin role{me.role ? ` (you are ${me.role})` : ''}.</div>
  </div>
{:else}
  <div class="panel" style="margin-bottom: 14px">
    {#if error}
      <div class="state error">Failed to load webhooks: {error}</div>
    {:else if !hooks}
      <div class="skeleton" style="height: 120px"></div>
    {:else if hooks.length === 0}
      <div class="state">No webhooks — create one below to receive signed run events.</div>
    {:else}
      <table class="data">
        <thead>
          <tr><th>Name</th><th>URL</th><th>Events</th><th>Status</th><th>Created</th><th></th></tr>
        </thead>
        <tbody>
          {#each hooks as w (w.id)}
            <tr>
              <td>{w.name}</td>
              <td class="mono dim">{w.url}</td>
              <td>{w.events.length === 0 ? 'all' : w.events.join(', ')}</td>
              <td><span class="badge {w.enabled ? 'ok' : 'neutral'}">{w.enabled ? 'enabled' : 'disabled'}</span></td>
              <td title={fmtTs(w.created_at_ns)}>{fmtAgo(w.created_at_ns)}</td>
              <td class="actions">
                <button class="btn" onclick={() => toggleEnabled(w)} disabled={busy}>
                  {w.enabled ? 'Disable' : 'Enable'}
                </button>
                <button class="btn danger" onclick={() => confirmDelete(w)} disabled={busy}>
                  {deleteArmedId === w.id ? 'Confirm?' : 'Delete'}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if rowError}<div class="state error" style="margin-top: 8px">{rowError}</div>{/if}
    {/if}
  </div>

  <div class="panel">
    <h2>New webhook</h2>
    <form
      class="create"
      onsubmit={(e) => {
        e.preventDefault();
        create();
      }}
    >
      <label>
        Name
        <input type="text" bind:value={newName} placeholder="ci sink" autocomplete="off" required />
      </label>
      <label>
        URL (https)
        <input type="text" bind:value={newUrl} placeholder="https://hooks.example.com/endpoint" required />
      </label>
      <label>
        Events (optional)
        <input type="text" bind:value={newEvents} placeholder="comma-separated; empty = all" />
      </label>
      <div>
        <button class="primary" type="submit" disabled={creating || !newName.trim() || !newUrl.trim()}>
          {creating ? 'Creating…' : 'Create webhook'}
        </button>
      </div>
    </form>
    <div class="hint">
      Payloads are signed X-Tumult-Signature (HMAC-SHA256). Deliveries are fire-and-log:
      a down receiver misses events rather than blocking runs.
    </div>
    {#if createError}<div class="state error" style="margin-top: 10px">{createError}</div>{/if}
    {#if oneTime}
      <div class="onetime">
        Signing secret for <b>{oneTime.name}</b> — shown once, never stored in the UI:
        <div class="mono pw">{oneTime.secret}</div>
      </div>
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
  .dim {
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
    font-size: 12px;
  }
  .create {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
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
  .onetime {
    margin-top: 14px;
    border: 1px solid var(--warn);
    border-radius: 6px;
    padding: 10px 12px;
    font-size: 13px;
  }
  .pw {
    font-size: 14px;
    user-select: all;
    margin-top: 6px;
  }
</style>

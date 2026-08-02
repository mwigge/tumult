<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtAgo, fmtTs } from '$lib/api';
  import type { GameDayEntry, MeResponse } from '$lib/types';

  let me = $state<MeResponse | null>(null);
  let days = $state<GameDayEntry[] | null>(null);
  let error = $state<string | null>(null);

  // Register form: the campaign TOON plus its experiment TOONs as a JSON
  // map of the campaign's path strings to TOON text.
  let toon = $state('');
  let experiments = $state('');
  let registerError = $state<string | null>(null);
  let registering = $state(false);

  // Registration is operator-level; the list is open to every role.
  const canMutate = $derived(
    me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false
  );

  onMount(() => {
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
    refresh();
  });

  async function refresh() {
    try {
      const r = await api.gamedays();
      days = r.gamedays;
      error = null;
    } catch (e) {
      if (!days) error = String(e);
    }
  }

  async function register() {
    if (registering) return;
    registering = true;
    registerError = null;
    try {
      let map: Record<string, string>;
      try {
        map = experiments.trim() === '' ? {} : JSON.parse(experiments);
      } catch {
        throw new Error('experiments must be a JSON object, e.g. {"pg-pause.toon": "title: …"}');
      }
      const r = await api.validateGameday(toon, map);
      toon = '';
      experiments = '';
      await refresh();
      window.location.href = `/gamedays/${r.gameday_registry_id}`;
    } catch (e) {
      registerError = String(e);
    } finally {
      registering = false;
    }
  }
</script>

<div class="page-head">
  <h1>GameDays</h1>
  <span class="sub">
    {days ? `${days.length} campaign${days.length === 1 ? '' : 's'}` : 'coordinated experiment campaigns'}
  </span>
</div>

<div class="panel" style="margin-bottom: 14px">
  {#if error}
    <div class="state error">Failed to load gamedays: {error}</div>
  {:else if !days}
    <div class="skeleton" style="height: 140px"></div>
  {:else if days.length === 0}
    <div class="state">No gamedays registered yet — register one below.</div>
  {:else}
    <table class="data">
      <thead>
        <tr><th>Title</th><th>ID</th><th>Registered</th><th>By</th></tr>
      </thead>
      <tbody>
        {#each days as d (d.id)}
          <tr class="clickable" onclick={() => (window.location.href = `/gamedays/${d.id}`)}>
            <td>{d.name}</td>
            <td class="mono" style="color: var(--text-dim)">{d.id}</td>
            <td title={fmtTs(d.registered_at_ns)}>{fmtAgo(d.registered_at_ns)}</td>
            <td>{d.registered_by ?? '—'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

{#if canMutate}
  <div class="panel">
    <h2>Register a gameday</h2>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        register();
      }}
    >
      <label>
        Campaign TOON
        <textarea
          rows="8"
          bind:value={toon}
          placeholder={'title: Q3 drill\nexperiments[1]:\n  - path: cpu-stress.toon\n    compliance_maps[0]:'}
          required
        ></textarea>
      </label>
      <label>
        Experiment TOONs (JSON map of path → TOON)
        <textarea
          rows="6"
          bind:value={experiments}
          placeholder={'{"cpu-stress.toon": "title: cpu stress\\nmethod[1]:\\n  - name: stress\\n    activity_type: action\\n    provider:\\n      type: native\\n      plugin: stress\\n      function: cpu"}'}
        ></textarea>
      </label>
      <div>
        <button class="primary" type="submit" disabled={registering || !toon.trim()}>
          {registering ? 'Registering…' : 'Validate and register'}
        </button>
      </div>
    </form>
    {#if registerError}<div class="state error" style="margin-top: 10px">{registerError}</div>{/if}
  </div>
{/if}

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  textarea {
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    color: var(--text);
    border-radius: 5px;
    padding: 8px 10px;
    font-family: var(--mono, monospace);
    font-size: 12.5px;
    resize: vertical;
  }
  form div {
    margin-top: 4px;
  }
</style>

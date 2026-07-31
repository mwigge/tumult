<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api, fmtAgo, fmtTs, shortId } from '$lib/api';
  import type {
    DryRunResponse,
    DryRunStep,
    MeResponse,
    RegistryDefinition,
    RegistryEntry
  } from '$lib/types';

  let me = $state<MeResponse | null>(null);
  let entries = $state<RegistryEntry[] | null>(null);
  let listError = $state<string | null>(null);

  let selectedId = $state<string | null>(null);
  let def = $state<RegistryDefinition | null>(null);
  let defError = $state<string | null>(null);

  let vars: Record<string, string> = $state({});
  let dry = $state<DryRunResponse | null>(null);
  // Identity of the definition+vars the last dry-run validated; the start
  // button is armed only while the current inputs still match it.
  let dryKey = $state<string | null>(null);
  let dryRunning = $state(false);
  let dryError = $state<string | null>(null);
  let starting = $state(false);
  let startError = $state<string | null>(null);

  // Operators and up may launch runs; missing role counts as viewer. When
  // the daemon has no users (open local mode) anyone may run.
  const canRun = $derived(
    me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false
  );
  const readOnlyNote = $derived(
    me?.auth_required && (!me.authenticated || !me.role || me.role === 'viewer')
  );

  /**
   * `${var}` placeholders in the definition TOON, in order of appearance,
   * deduped. Escaped `$${var}` and namespaced `${config.*}` / `${secrets.*}`
   * placeholders are not user parameters — they are skipped.
   */
  const placeholders: string[] = $derived.by(() => {
    if (!def) return [];
    const out: string[] = [];
    const seen = new Set<string>();
    for (const m of def.definition_toon.matchAll(/(?<!\$)\$\{([A-Za-z_][A-Za-z0-9_.]*)\}/g)) {
      const name = m[1];
      if (name.startsWith('config.') || name.startsWith('secrets.')) continue;
      if (!seen.has(name)) {
        seen.add(name);
        out.push(name);
      }
    }
    return out;
  });

  const allFilled = $derived(placeholders.every((p) => (vars[p] ?? '').trim() !== ''));

  const currentKey = $derived(
    selectedId ? JSON.stringify([selectedId, ...placeholders.map((p) => vars[p] ?? '')]) : null
  );
  const dryValidForCurrent = $derived(
    dry !== null && dry.valid && dryKey !== null && dryKey === currentKey
  );

  onMount(() => {
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
    api
      .registry()
      .then((r) => (entries = r.definitions))
      .catch((e) => (listError = String(e)));
  });

  async function select(id: string) {
    if (id === selectedId) return;
    selectedId = id;
    def = null;
    defError = null;
    dry = null;
    dryKey = null;
    startError = null;
    try {
      const r = await api.registryDefinition(id);
      if (selectedId !== id) return; // selection moved on while loading
      def = r.definition;
      vars = {};
    } catch (e) {
      if (selectedId === id) defError = String(e);
    }
  }

  async function runDry() {
    if (!selectedId || !allFilled || dryRunning) return;
    dryRunning = true;
    dryError = null;
    startError = null;
    try {
      const payload = Object.fromEntries(placeholders.map((p) => [p, vars[p] ?? '']));
      dry = await api.dryRun(selectedId, payload);
      dryKey = JSON.stringify([selectedId, ...placeholders.map((p) => vars[p] ?? '')]);
    } catch (e) {
      dry = null;
      dryKey = null;
      dryError = String(e);
    } finally {
      dryRunning = false;
    }
  }

  async function start() {
    if (!selectedId || !dryValidForCurrent || !canRun || starting) return;
    starting = true;
    startError = null;
    try {
      const payload = Object.fromEntries(placeholders.map((p) => [p, vars[p] ?? '']));
      const r = await api.startRun(selectedId, payload);
      // Gated runs land in pending_approval instead of starting; the detail
      // page shows the "awaiting approval" notice with a link to the queue.
      const qs = r.state === 'pending_approval' ? `?awaiting=${r.tier ?? ''}` : '';
      await goto(`/runs/${r.run_id}${qs}`);
    } catch (e) {
      startError = String(e);
      starting = false;
    }
  }

  function stepTimeout(s: DryRunStep): string {
    const t = s.timeout_s ?? s.provider?.timeout_s;
    return t !== null && t !== undefined ? `${t}s` : '—';
  }
</script>

<div class="page-head">
  <a href="/runs" style="color: var(--text-dim)">← runs</a>
  <h1>New run</h1>
  {#if readOnlyNote}
    <span class="sub">your role ({me?.role ?? 'viewer'}) is read-only here</span>
  {/if}
</div>

<div class="panel" style="margin-bottom: 14px">
  <h2>1 · Definition</h2>
  {#if listError}
    <div class="state error">Failed to load the registry: {listError}</div>
  {:else if !entries}
    <div class="skeleton" style="height: 120px"></div>
  {:else if entries.length === 0}
    <div class="state">
      No validated definitions yet — validate an experiment TOON first (POST /api/runs/validate).
      Registration happens via the API or the CLI (<span class="mono">tumult run --validate</span>).
    </div>
  {:else}
    <table class="data">
      <thead>
        <tr><th></th><th>Name</th><th>ID</th><th>Hash</th><th>Registered by</th><th>Registered</th></tr>
      </thead>
      <tbody>
        {#each entries as e (e.id)}
          <tr class="clickable" class:selected={selectedId === e.id} onclick={() => select(e.id)}>
            <td style="color: var(--accent)">{selectedId === e.id ? '●' : '○'}</td>
            <td>{e.name}</td>
            <td class="mono" style="color: var(--text-dim)">{e.id}</td>
            <td class="mono" style="color: var(--text-dim)">{shortId(e.content_hash)}</td>
            <td>{e.registered_by ?? '—'}</td>
            <td title={fmtTs(e.registered_at_ns)}>{fmtAgo(e.registered_at_ns)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

{#if selectedId}
  <div class="panel" style="margin-bottom: 14px">
    <h2>2 · Parameters</h2>
    {#if defError}
      <div class="state error">Failed to load the definition: {defError}</div>
    {:else if !def}
      <div class="skeleton" style="height: 60px"></div>
    {:else if placeholders.length === 0}
      <div class="state">This definition has no parameters.</div>
    {:else}
      <div class="params">
        {#each placeholders as p (p)}
          <label>
            <span class="mono">{p}</span>
            <input
              type="text"
              value={vars[p] ?? ''}
              disabled={!canRun}
              oninput={(e) => (vars[p] = e.currentTarget.value)}
            />
          </label>
        {/each}
      </div>
      {#if !allFilled}
        <div class="hint">All parameters are required before the plan can be dry-run.</div>
      {/if}
    {/if}
  </div>

  <div class="panel" style="margin-bottom: 14px">
    <h2>3 · Dry-run preview</h2>
    <button class="primary" onclick={runDry} disabled={!def || !allFilled || dryRunning}>
      {dryRunning ? 'Resolving…' : 'Dry run'}
    </button>
    {#if dryError}
      <div class="state error">Dry run failed: {dryError}</div>
    {:else if dry && !dry.valid}
      <div class="state error">Definition does not resolve with these parameters: {dry.error}</div>
    {:else if dry && dry.valid}
      {@const plan = dry.plan}
      <div class="plan">
        <div class="plan-head">
          <b>{plan.title || def?.name}</b>
          {#each plan.tags ?? [] as tag (tag)}
            <span class="badge neutral">{tag}</span>
          {/each}
        </div>
        {#if plan.description}
          <p class="desc">{plan.description}</p>
        {/if}
        {#if plan.estimate}
          <div class="estimate">
            Estimate: {plan.estimate.expected_outcome ?? '—'}{plan.estimate.expected_recovery_s != null
              ? ` · recovery ~${plan.estimate.expected_recovery_s}s`
              : ''}{plan.estimate.confidence ? ` · confidence ${plan.estimate.confidence}` : ''}
          </div>
        {/if}
        {#if plan.hypothesis}
          <h3>Hypothesis — {plan.hypothesis.title}</h3>
          {#if plan.hypothesis.probes.length === 0}
            <div class="hint">No probes.</div>
          {:else}
            <ul>
              {#each plan.hypothesis.probes as probe, i (i)}
                <li class="mono">{probe.name}</li>
              {/each}
            </ul>
          {/if}
        {/if}
        <h3>Method ({plan.method.length} step{plan.method.length === 1 ? '' : 's'})</h3>
        {#if plan.method.length === 0}
          <div class="hint">No method steps.</div>
        {:else}
          <ol>
            {#each plan.method as step, i (i)}
              <li>
                <span class="mono">{step.name}</span>
                <span class="meta mono">{step.provider?.type ?? '?'} · {step.activity_type} · timeout {stepTimeout(step)}</span>
              </li>
            {/each}
          </ol>
        {/if}
        <h3>Rollbacks ({plan.rollbacks.length})</h3>
        {#if plan.rollbacks.length === 0}
          <div class="hint">No rollback steps.</div>
        {:else}
          <ol>
            {#each plan.rollbacks as step, i (i)}
              <li>
                <span class="mono">{step.name}</span>
                <span class="meta mono">{step.provider?.type ?? '?'} · {step.activity_type} · timeout {stepTimeout(step)}</span>
              </li>
            {/each}
          </ol>
        {/if}
      </div>
    {/if}
  </div>

  <div class="panel">
    <h2>4 · Execute</h2>
    {#if !canRun}
      <div class="hint">Starting runs requires the operator role or above.</div>
    {:else if dry && dry.valid && !dryValidForCurrent}
      <div class="hint">Parameters changed since the dry run — dry run again before starting.</div>
    {:else if !dryValidForCurrent}
      <div class="hint">A valid dry run for the current parameters is required before starting.</div>
    {/if}
    <button class="primary" onclick={start} disabled={!dryValidForCurrent || !canRun || starting}>
      {starting ? 'Starting…' : 'Start run'}
    </button>
    {#if startError}
      <div class="state error">Failed to start the run: {startError}</div>
    {/if}
  </div>
{/if}

<style>
  tr.selected td {
    background: var(--bg-hover);
  }
  .params {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 10px 18px;
  }
  .params label span {
    display: block;
    color: var(--text-dim);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    margin-bottom: 4px;
  }
  .params input {
    width: 100%;
  }
  .hint {
    color: var(--text-dim);
    font-size: 12.5px;
    margin-top: 8px;
  }
  .plan {
    margin-top: 12px;
  }
  .plan-head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .desc {
    color: var(--text-dim);
    font-size: 13px;
    margin: 6px 0;
  }
  .estimate {
    color: var(--text-dim);
    font-size: 12.5px;
    margin-bottom: 8px;
  }
  .plan h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    margin: 12px 0 4px;
  }
  .plan ul,
  .plan ol {
    margin: 4px 0;
    padding-left: 22px;
  }
  .plan li {
    padding: 2px 0;
    font-size: 13px;
  }
  .plan li .meta {
    color: var(--text-faint);
    font-size: 12px;
    margin-left: 8px;
  }
</style>

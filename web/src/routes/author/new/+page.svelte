<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { api } from '$lib/api';
  import ActionParamForm, { emptyForm, type AuthorForm } from '$lib/components/ActionParamForm.svelte';
  import ToonEditor from '$lib/components/ToonEditor.svelte';
  import type {
    CatalogAction,
    ScaffoldRequest,
    ScaffoldResponse,
    ValidateToonResponse
  } from '$lib/types';

  const plugin = $derived($page.url.searchParams.get('plugin') ?? '');
  const actionName = $derived($page.url.searchParams.get('action') ?? '');

  let action = $state<CatalogAction | null>(null);
  let loadError = $state<string | null>(null);

  let form: AuthorForm = $state(emptyForm());
  let toon = $state('');
  let scaffoldResult = $state<ScaffoldResponse | null>(null);
  let scaffolding = $state(false);
  let scaffoldError = $state<string | null>(null);
  let registered = $state<ValidateToonResponse | null>(null);

  onMount(() => {
    api
      .catalog()
      .then((c) => {
        const found = c.domains
          .flatMap((d) => d.actions)
          .find((a) => a.plugin === plugin && a.name === actionName);
        if (found) {
          action = found;
        } else {
          loadError = `unknown action ${plugin}::${actionName}`;
        }
      })
      .catch((e) => (loadError = String(e)));
  });

  const missingRequired = $derived(
    form.target.trim() === '' ||
      (action?.args ?? []).some((a) => a.required && (form.args[a.name] ?? '').trim() === '')
  );

  async function generate() {
    if (!action || missingRequired || scaffolding) return;
    scaffolding = true;
    scaffoldError = null;
    registered = null;
    try {
      const args = Object.fromEntries(
        Object.entries(form.args).filter(([, v]) => v.trim() !== '')
      );
      const req: ScaffoldRequest = {
        plugin: action.plugin,
        action: action.name,
        args,
        target: form.target.trim()
      };
      if (form.title.trim() !== '') req.title = form.title.trim();
      if (form.probeKind === 'http' && form.probeUrl.trim() !== '') {
        req.probe_url = form.probeUrl.trim();
      } else if (form.probeKind === 'exec' && form.probeCommand.trim() !== '') {
        req.probe_command = form.probeCommand.trim();
      }
      if (form.probeExpect.trim() !== '') req.probe_expect = form.probeExpect.trim();
      scaffoldResult = await api.scaffold(req);
      toon = scaffoldResult.toon;
    } catch (e) {
      scaffoldResult = null;
      scaffoldError = String(e);
    } finally {
      scaffolding = false;
    }
  }
</script>

<div class="page-head">
  <a href="/author" style="color: var(--text-dim)">← catalog</a>
  <h1>Author an experiment</h1>
</div>

{#if loadError}
  <div class="state error">Failed to load the action: {loadError}</div>
{:else if !action}
  <div class="skeleton" style="height: 160px"></div>
{:else}
  <div class="panel" style="margin-bottom: 14px">
    <h2>1 · Action</h2>
    <div class="action-line">
      <span class="mono">{action.plugin}::{action.name}</span>
      <span class="badge {action.kind === 'action' ? 'warn' : 'neutral'}">{action.kind}</span>
    </div>
    <p class="desc">{action.description}</p>
  </div>

  <div class="panel" style="margin-bottom: 14px">
    <h2>2 · Parameters</h2>
    <ActionParamForm {action} bind:form />
    {#if missingRequired}
      <div class="hint">Target and all required arguments are needed before generating.</div>
    {/if}
  </div>

  <div class="panel" style="margin-bottom: 14px">
    <h2>3 · Experiment TOON</h2>
    <button class="primary" onclick={generate} disabled={missingRequired || scaffolding}>
      {scaffolding ? 'Generating…' : toon ? 'Regenerate TOON' : 'Generate TOON'}
    </button>
    {#if scaffoldError}
      <div class="state error">Scaffold failed: {scaffoldError}</div>
    {:else if scaffoldResult && !scaffoldResult.valid}
      <div class="state error">
        The generated experiment does not validate: {scaffoldResult.validation_error ?? 'unknown error'}
        — edit the TOON below to fix it.
      </div>
    {/if}
    {#if toon}
      <div style="margin-top: 12px">
        <ToonEditor bind:toon bind:result={registered} />
      </div>
    {/if}
  </div>

  {#if registered && registered.valid}
    <div class="panel">
      <h2>4 · Launch</h2>
      <p class="desc">
        <span class="mono">{registered.registry_id}</span> is registered — dry-run the resolved
        plan and start the run from the launcher.
      </p>
      <button
        class="primary"
        onclick={() => goto(`/runs/new?registry_id=${encodeURIComponent(registered?.valid ? registered.registry_id : '')}`)}
      >
        Dry run & launch →
      </button>
    </div>
  {/if}
{/if}

<style>
  .action-line {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .desc {
    color: var(--text-dim);
    font-size: 13px;
    margin: 6px 0;
  }
  .hint {
    color: var(--text-dim);
    font-size: 12.5px;
    margin-top: 8px;
  }
</style>

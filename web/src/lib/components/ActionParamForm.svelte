<script lang="ts" module>
  /** Draft state for the action parameter form. */
  export interface AuthorForm {
    title: string;
    target: string;
    args: Record<string, string>;
    probeKind: 'exec' | 'http';
    probeCommand: string;
    probeUrl: string;
    probeExpect: string;
  }

  export function emptyForm(): AuthorForm {
    return {
      title: '',
      target: '',
      args: {},
      probeKind: 'exec',
      probeCommand: '',
      probeUrl: '',
      probeExpect: ''
    };
  }
</script>

<script lang="ts">
  import type { CatalogAction } from '$lib/types';

  let { action, form = $bindable() }: { action: CatalogAction; form: AuthorForm } = $props();
</script>

<div class="grid cols-2">
  <label>
    <span>Title <i>(optional)</i></span>
    <input type="text" placeholder="{action.name} — <target>" bind:value={form.title} />
  </label>
  <label>
    <span>Target <b class="req">*</b></span>
    <input type="text" placeholder="e.g. demo-postgres" bind:value={form.target} />
  </label>
</div>

{#if action.args.length > 0}
  <div class="grid cols-2">
    {#each action.args as arg (arg.name)}
      <label>
        <span class="mono">{arg.name}{#if arg.required} <b class="req">*</b>{/if}</span>
        <input
          type="text"
          placeholder={arg.description}
          value={form.args[arg.name] ?? ''}
          oninput={(e) => (form.args[arg.name] = e.currentTarget.value)}
        />
      </label>
    {/each}
  </div>
{/if}

<h3>Steady-state probe</h3>
<div class="seg" role="group" aria-label="probe kind">
  <button class:active={form.probeKind === 'exec'} onclick={() => (form.probeKind = 'exec')}>
    Command
  </button>
  <button class:active={form.probeKind === 'http'} onclick={() => (form.probeKind = 'http')}>
    HTTP URL
  </button>
</div>
<div class="grid cols-2" style="margin-top: 10px">
  {#if form.probeKind === 'exec'}
    <label>
      <span>Probe command <i>(optional — a default health check is used)</i></span>
      <input type="text" placeholder="pg_isready -h demo-postgres" bind:value={form.probeCommand} />
    </label>
  {:else}
    <label>
      <span>Probe URL <i>(optional — a default health check is used)</i></span>
      <input type="text" placeholder="http://demo-app:8080/health" bind:value={form.probeUrl} />
    </label>
  {/if}
  <label>
    <span>Expected output <i>(regex, optional)</i></span>
    <input type="text" placeholder="accepting connections" bind:value={form.probeExpect} />
  </label>
</div>

<style>
  label span {
    display: block;
    color: var(--text-dim);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    margin-bottom: 4px;
  }
  label span i {
    text-transform: none;
    letter-spacing: 0;
  }
  label input {
    width: 100%;
  }
  .req {
    color: var(--warn);
  }
  h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    margin: 12px 0 6px;
  }
</style>

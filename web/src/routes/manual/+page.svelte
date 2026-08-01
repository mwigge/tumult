<script lang="ts">
  // Manual evidence: enter hand-executed test records under attestation,
  // and verify/reject submitted records (reviewer ≠ enterer — enforced by
  // the API). There is no auth yet: the "acting as" name is a plain string.
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import ManualEntryTab, { emptyForm } from '$lib/components/ManualEntryTab.svelte';
  import ManualReviewTab from '$lib/components/ManualReviewTab.svelte';

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

  // Entry-tab state lives here so a half-filled draft (and its result
  // message) survives switching to another tab and back.
  let form = $state(emptyForm());
  let entryMsg: { ok: boolean; text: string } | null = $state(null);
  let busy = $state(false);

  // Review notes, keyed by record id; kept here for the same reason.
  let notes: Record<string, string> = $state({});
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
  <ManualEntryTab {actor} bind:form bind:entryMsg bind:busy />
{:else}
  <ManualReviewTab {tab} {actor} bind:notes />
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
</style>

<script lang="ts">
  import { api } from '$lib/api';
  import type { AskResponse } from '$lib/types';

  const EXAMPLES = [
    'How many experiments ran?',
    'What is the pass rate?',
    'Which experiments failed?',
    'What faults were injected?',
    'Experiments per day',
    'Show recent experiments'
  ];

  let question = $state('');
  let loading = $state(false);
  let result: AskResponse | null = $state(null);
  let error: string | null = $state(null);

  async function submit(q: string) {
    question = q;
    if (!q.trim()) return;
    loading = true;
    result = null;
    error = null;
    try {
      result = await api.ask(q);
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  const columns: string[] = $derived.by(() => {
    const rows = result?.rows;
    if (!rows || rows.length === 0) return [];
    return [...new Set(rows.flatMap((r) => Object.keys(r)))];
  });

  function cell(v: unknown): string {
    if (v === null || v === undefined) return '—';
    if (typeof v === 'object') return JSON.stringify(v);
    return String(v);
  }
</script>

<div class="page-head">
  <h1>Ask</h1>
  <span class="sub">natural-language analytics over the telemetry store — guarded SQL, read-only</span>
</div>

<div class="panel" style="margin-bottom: 14px">
  <form
    class="askbar"
    onsubmit={(e) => {
      e.preventDefault();
      submit(question);
    }}
  >
    <input
      type="text"
      bind:value={question}
      placeholder="Ask about experiments, outcomes, faults…"
      style="flex: 1"
    />
    <button class="primary" type="submit" disabled={loading || !question.trim()}>
      {loading ? 'Thinking…' : 'Ask'}
    </button>
  </form>
  <div class="examples">
    {#each EXAMPLES as ex (ex)}
      <button onclick={() => submit(ex)}>{ex}</button>
    {/each}
  </div>
</div>

{#if error}
  <div class="state error panel">{error}</div>
{:else if loading}
  <div class="skeleton" style="height: 160px"></div>
{:else if result}
  {#if !result.configured}
    <div class="panel state" style="text-align: left">
      <b>AI analytics is not configured.</b><br />
      Point kronikad at an OpenAI-compatible endpoint (Ollama works out of the box):<br /><br />
      <code>KRONIKA_LLM_BASE_URL=http://localhost:11434/v1 KRONIKA_LLM_MODEL=qwen2.5:7b kronikad</code><br /><br />
      The example questions above are answered from a curated golden bank and work
      without an LLM — try one.
    </div>
  {:else}
    <div class="panel" style="margin-bottom: 14px">
      <h2>Generated SQL {result.source === 'golden' ? '(curated answer)' : '(LLM)'}</h2>
      <pre class="mono sql">{result.sql}</pre>
      {#if result.error}
        <div class="state error">{result.error}</div>
      {/if}
    </div>
    {#if result.rows}
      <div class="panel">
        <h2>Result — {result.rows.length} row{result.rows.length === 1 ? '' : 's'}</h2>
        {#if result.rows.length === 0}
          <div class="state">The query returned no rows.</div>
        {:else}
          <div style="overflow-x: auto">
            <table class="data">
              <thead>
                <tr>
                  {#each columns as c (c)}<th>{c}</th>{/each}
                </tr>
              </thead>
              <tbody>
                {#each result.rows as row, i (i)}
                  <tr>
                    {#each columns as c (c)}<td class="mono">{cell(row[c])}</td>{/each}
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>
    {/if}
  {/if}
{/if}

<style>
  .askbar {
    display: flex;
    gap: 10px;
  }
  .examples {
    margin-top: 10px;
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .examples button {
    background: var(--bg-hover);
    border: 1px solid var(--border-strong);
    color: var(--text-dim);
    border-radius: 12px;
    padding: 3px 11px;
    font-size: 12px;
    cursor: pointer;
  }
  .examples button:hover {
    color: var(--text);
    border-color: var(--accent);
  }
  .sql {
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 10px 12px;
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--accent);
  }
</style>

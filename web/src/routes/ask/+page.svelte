<script lang="ts">
    import { api } from '#lib/api.js';
    import type { AskResponse } from '#lib/types.js';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';

    const EXAMPLES = [
        'How many experiments ran?',
        'What is the pass rate?',
        'Which experiments failed?',
        'What faults were injected?',
        'Experiments per day',
        'Show recent experiments',
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
            error =
                (e as { status?: number }).status === 422
                    ? "Your access is limited to specific environments, and this question needs data that isn't tied to one. Try a question about a single environment, or ask an admin to widen your scopes."
                    : String(e);
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

<PageHeader
    title="Ask"
    sub="natural-language analytics over the telemetry store — guarded SQL, read-only"
/>

<Panel style="margin-bottom: var(--space-3-5)">
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
</Panel>

{#if error}
    <State kind="error" panel>{error}</State>
{:else if loading}
    <State kind="loading" height={160} />
{:else if result}
    {#if !result.configured}
        <State kind="empty" panel style="text-align: left">
            <b>AI analytics is not configured.</b><br />
            Point tumultd at an OpenAI-compatible endpoint (Ollama works out of the box):<br /><br
            />
            <code
                >KRONIKA_LLM_BASE_URL=http://localhost:11434/v1 KRONIKA_LLM_MODEL=qwen2.5:7b tumultd</code
            ><br /><br />
            The example questions above are answered from a curated golden bank and work without an LLM
            — try one.
        </State>
    {:else}
        <Panel
            title={`Generated SQL ${result.source === 'golden' ? '(curated answer)' : '(LLM)'}`}
            style="margin-bottom: var(--space-3-5)"
        >
            <pre class="mono sql">{result.sql}</pre>
            {#if result.error}
                <State kind="error">{result.error}</State>
            {/if}
        </Panel>
        {#if result.rows}
            <Panel
                title={`Result — ${result.rows.length} row${result.rows.length === 1 ? '' : 's'}`}
            >
                {#if result.rows.length === 0}
                    <State kind="empty">The query returned no rows.</State>
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
                                        {#each columns as c (c)}<td class="mono">{cell(row[c])}</td
                                            >{/each}
                                    </tr>
                                {/each}
                            </tbody>
                        </table>
                    </div>
                {/if}
            </Panel>
        {/if}
    {/if}
{/if}

<style>
    .askbar {
        display: flex;
        gap: var(--space-2-5);
    }
    .examples {
        margin-top: var(--space-2-5);
        display: flex;
        gap: var(--space-2);
        flex-wrap: wrap;
    }
    .examples button {
        background: var(--bg-hover);
        border: var(--border-width-sm) solid var(--border-strong);
        color: var(--text-dim);
        border-radius: 12px;
        padding: 3px 11px;
        font-size: var(--font-size-xs);
        cursor: pointer;
    }
    .examples button:hover {
        color: var(--text);
        border-color: var(--accent);
    }
    .sql {
        background: var(--bg-raised);
        border: var(--border-width-sm) solid var(--border);
        border-radius: var(--radius-md);
        padding: var(--space-2-5) var(--space-3);
        font-size: var(--font-size-xs);
        white-space: pre-wrap;
        word-break: break-word;
        color: var(--accent);
    }
</style>

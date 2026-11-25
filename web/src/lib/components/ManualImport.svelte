<script lang="ts">
    // Import records as drafts, in one batch: the JSON body that
    // `POST /api/manual/import` takes, or a list of records. Every record
    // still needs a submit and its sign-offs to count.
    import { api } from '#lib/api.js';
    import { errorText, importBody } from '#lib/manual.js';
    import Panel from './Panel.svelte';
    import State from './State.svelte';

    let { actor, canWrite }: { actor: string; canWrite: boolean } = $props();

    let raw = $state('');
    let label = $state('');
    let busy = $state(false);
    let message: { ok: boolean; text: string } | null = $state(null);
    let ids: string[] = $state([]);

    const plural = (n: number) => `${n} draft${n === 1 ? '' : 's'}`;

    async function load(input: HTMLInputElement) {
        const chosen = input.files?.[0];
        if (chosen) raw = await chosen.text();
    }

    async function send() {
        message = null;
        ids = [];
        const parsed = importBody(raw, label, actor);
        if ('error' in parsed) {
            message = { ok: false, text: parsed.error };
            return;
        }
        busy = true;
        try {
            const done = await api.manualImport(parsed.body);
            ids = done.ids;
            message = {
                ok: true,
                text: `Imported ${plural(done.ids.length)} in batch ${done.batch_id}.`,
            };
        } catch (e) {
            message = { ok: false, text: errorText(e) };
        } finally {
            busy = false;
        }
    }
</script>

<Panel title="Import records">
    {#if !canWrite}
        <p>An operator can import records.</p>
    {:else}
        <div class="form">
            <p class="note">
                The body that <code>POST /api/manual/import</code> takes, or a list of records with the
                fields of the form. Each record becomes a draft; a field the API does not know is refused,
                and nothing of the batch is kept.
            </p>
            <div class="row2">
                <label
                    >JSON file <input type="file" onchange={(e) => load(e.currentTarget)} /></label
                >
                <label>Batch label <input type="text" bind:value={label} /></label>
            </div>
            <label
                >Records as JSON
                <textarea rows="12" class="mono" bind:value={raw}></textarea>
            </label>
            <div class="actions">
                <button class="btn primary" disabled={busy} onclick={send}>Import as drafts</button>
            </div>
            {#if message}
                <State kind={message.ok ? 'empty' : 'error'} role="status">{message.text}</State>
            {/if}
            {#if ids.length > 0}
                <ul class="mono">
                    {#each ids as id (id)}<li>{id}</li>{/each}
                </ul>
            {/if}
        </div>
    {/if}
</Panel>

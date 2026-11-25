<script lang="ts">
    import { onMount } from 'svelte';
    import { api, fmtAgo, fmtTs } from '#lib/api.js';
    import type { MeResponse, Webhook } from '#lib/types.js';

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
        api.me()
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
        {hooks
            ? `${hooks.length} sink${hooks.length === 1 ? '' : 's'}`
            : 'signed run-event notifications'}
    </span>
</div>

{#if me && !isAdmin}
    <div class="panel">
        <div class="state">
            Webhook management requires the admin role{me.role ? ` (you are ${me.role})` : ''}.
        </div>
    </div>
{:else}
    <div class="panel" style="margin-bottom: var(--space-3-5)">
        {#if error}
            <div class="state error">Failed to load webhooks: {error}</div>
        {:else if !hooks}
            <div class="skeleton" style="height: 120px"></div>
        {:else if hooks.length === 0}
            <div class="state">No webhooks — create one below to receive signed run events.</div>
        {:else}
            <table class="data">
                <thead>
                    <tr
                        ><th>Name</th><th>URL</th><th>Events</th><th>Status</th><th>Created</th><th
                        ></th></tr
                    >
                </thead>
                <tbody>
                    {#each hooks as w (w.id)}
                        <tr>
                            <td>{w.name}</td>
                            <td class="mono dim">{w.url}</td>
                            <td>{w.events.length === 0 ? 'all' : w.events.join(', ')}</td>
                            <td
                                ><span class="badge {w.enabled ? 'ok' : 'neutral'}"
                                    >{w.enabled ? 'enabled' : 'disabled'}</span
                                ></td
                            >
                            <td title={fmtTs(w.created_at_ns)}>{fmtAgo(w.created_at_ns)}</td>
                            <td class="actions">
                                <button
                                    class="btn"
                                    onclick={() => toggleEnabled(w)}
                                    disabled={busy}
                                >
                                    {w.enabled ? 'Disable' : 'Enable'}
                                </button>
                                <button
                                    class="btn danger"
                                    onclick={() => confirmDelete(w)}
                                    disabled={busy}
                                >
                                    {deleteArmedId === w.id ? 'Confirm?' : 'Delete'}
                                </button>
                            </td>
                        </tr>
                    {/each}
                </tbody>
            </table>
            {#if rowError}<div class="state error" style="margin-top: var(--space-2)">
                    {rowError}
                </div>{/if}
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
                <input
                    type="text"
                    bind:value={newName}
                    placeholder="ci sink"
                    autocomplete="off"
                    required
                />
            </label>
            <label>
                URL (https)
                <input
                    type="text"
                    bind:value={newUrl}
                    placeholder="https://hooks.example.com/endpoint"
                    required
                />
            </label>
            <label>
                Events (optional)
                <input
                    type="text"
                    bind:value={newEvents}
                    placeholder="comma-separated; empty = all"
                />
            </label>
            <div>
                <button
                    class="primary"
                    type="submit"
                    disabled={creating || !newName.trim() || !newUrl.trim()}
                >
                    {creating ? 'Creating…' : 'Create webhook'}
                </button>
            </div>
        </form>
        <div class="hint">
            Payloads are signed X-Tumult-Signature (HMAC-SHA256). Deliveries are fire-and-log: a
            down receiver misses events rather than blocking runs.
        </div>
        {#if createError}<div class="state error" style="margin-top: var(--space-2-5)">
                {createError}
            </div>{/if}
        {#if oneTime}
            <div class="onetime">
                Signing secret for <b>{oneTime.name}</b> — shown once, never stored in the UI:
                <div class="mono pw">{oneTime.secret}</div>
            </div>
        {/if}
    </div>
{/if}

<style>
    .btn {
        white-space: nowrap;
    }
    .dim {
        max-width: 300px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        color: var(--text-dim);
        font-size: var(--font-size-xs);
    }
    .create {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
        gap: var(--space-3) var(--space-4-5);
        align-items: end;
    }
    .create label {
        display: flex;
        flex-direction: column;
        gap: 5px;
        font-size: var(--font-size-sm);
        color: var(--text-dim);
    }
    .hint {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
        margin-top: var(--space-2-5);
    }
    .onetime {
        margin-top: var(--space-3-5);
        border: var(--border-width-sm) solid var(--warn);
        border-radius: var(--radius-xl);
        padding: var(--space-2-5) var(--space-3);
        font-size: var(--font-size-md);
    }
    .pw {
        font-size: var(--font-size-xl);
        user-select: all;
        margin-top: var(--space-1-5);
    }
</style>

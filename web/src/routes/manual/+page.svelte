<script lang="ts">
    // Manual evidence: record DR exercises and other hand-executed tests
    // under attestation, plan them with due dates, attach evidence, sign
    // them off (a reviewer is not the enterer, enforced by the API) and
    // verify their audit trail. Authenticated attribution comes from the
    // session.
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { onMount } from 'svelte';
    import { api } from '#lib/api.js';
    import { detailToForm, emptyForm, errorText } from '#lib/manual.js';
    import type { MeResponse } from '#lib/types.js';
    import ManualEntryTab from '#lib/components/ManualEntryTab.svelte';
    import ManualImport from '#lib/components/ManualImport.svelte';
    import ManualPlansTab from '#lib/components/ManualPlansTab.svelte';
    import ManualReviewTab from '#lib/components/ManualReviewTab.svelte';

    const TABS = ['entry', 'plans', 'queue', 'records', 'import'] as const;
    const TAB_LABEL: Record<string, string> = {
        plans: 'Plans',
        queue: 'Verification queue',
        records: 'All records',
        import: 'Import',
    };

    const tab = $derived(page.url.searchParams.get('tab') ?? 'entry');

    let actor = $state('');
    let me = $state<MeResponse | null>(null);
    let editingId = $state<string | null>(null);
    const canWrite = $derived(
        !!me && (!me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer')),
    );
    const canReview = $derived(
        !!me &&
            (!me.auth_required ||
                (me.authenticated && (me.role === 'approver' || me.role === 'admin'))),
    );
    onMount(() => {
        api.me()
            .then((session) => {
                me = session;
                actor = session.auth_required
                    ? (session.username ?? '')
                    : (localStorage.getItem('kronika.actor') ?? '');
            })
            .catch((e) => {
                entryMsg = { ok: false, text: String(e) };
            });
    });
    function setActor(v: string) {
        actor = v;
        localStorage.setItem('kronika.actor', v);
    }

    function setTab(t: string) {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const params = new URLSearchParams(page.url.searchParams.toString());
        params.set('tab', t);
        goto(`?${params}`, { replace: true, reset: false });
    }

    // Entry-tab state lives here so a half-filled draft (and its result
    // message) survives switching to another tab and back.
    let form = $state(emptyForm());
    let entryMsg: { ok: boolean; text: string } | null = $state(null);
    let busy = $state(false);

    async function editDraft(id: string) {
        try {
            const detail = await api.manualDetail(id);
            if (detail.experiment.status !== 'draft') throw new Error('Only drafts can be edited.');
            form = detailToForm(detail);
            editingId = id;
            entryMsg = null;
            setTab('entry');
        } catch (e) {
            entryMsg = { ok: false, text: errorText(e) };
        }
    }
</script>

<div class="manual">
    <div class="page-head">
        <h1>Manual evidence</h1>
        <span class="sub"
            >DR exercises and other hand-executed tests, entered under attestation</span
        >
        <div class="controls">
            <label class="actor">
                {me?.auth_required ? 'Signed in as' : 'Acting as'}
                <input
                    type="text"
                    placeholder="your name"
                    value={actor}
                    readonly={me?.auth_required}
                    oninput={(e) => setActor(e.currentTarget.value)}
                />
            </label>
            <div class="seg" role="group" aria-label="manual sections">
                {#each TABS as t (t)}
                    <button class:active={tab === t} onclick={() => setTab(t)}>
                        {t === 'entry' ? (editingId ? 'Draft editor' : 'New entry') : TAB_LABEL[t]}
                    </button>
                {/each}
            </div>
        </div>
    </div>

    {#if entryMsg && !entryMsg.ok && (tab !== 'entry' || !canWrite)}<div class="state error">
            {entryMsg.text}
        </div>{/if}

    {#if tab === 'entry'}
        {#if canWrite}
            {#if editingId}<p>
                    Editing draft <code>{editingId}</code>.
                    <button
                        class="btn"
                        disabled={busy}
                        onclick={() => {
                            editingId = null;
                            form = emptyForm();
                            entryMsg = null;
                        }}>New entry</button
                    >
                </p>{/if}
            <ManualEntryTab
                {actor}
                allowedEnvironments={me?.env_scopes ?? []}
                bind:editingId
                bind:form
                bind:entryMsg
                bind:busy
            />
        {:else}<p>Your role can read manual evidence. An operator can create or edit it.</p>{/if}
    {:else if tab === 'plans'}
        <ManualPlansTab {actor} {canWrite} />
    {:else if tab === 'import'}
        <ManualImport {actor} {canWrite} />
    {:else}
        <ManualReviewTab {tab} {actor} {canReview} canEdit={canWrite} onedit={editDraft} />
    {/if}
</div>

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
    /* The parts of the manual components share one look: their forms, rows
       of fields, list editors and buttons. */
    .manual :global(.form) {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .manual :global(.form label) {
        display: flex;
        flex-direction: column;
        gap: 4px;
        font-size: 12px;
        color: var(--text-dim);
    }
    .manual :global(.row2),
    .manual :global(.row3) {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 10px;
    }
    .manual :global(.row3) {
        grid-template-columns: 1fr 1fr 1fr;
    }
    .manual :global(.grid-rows) {
        display: grid;
        gap: 6px;
        align-items: center;
        margin-bottom: 6px;
        font-size: 11.5px;
        color: var(--text-dim);
    }
    .manual :global(.grid-rows input),
    .manual :global(.grid-rows select) {
        min-width: 0;
        width: 100%;
    }
    .manual :global(.grid-rows .n) {
        color: var(--text-faint);
        text-align: right;
    }
    .manual :global(.note),
    .manual :global(.dim) {
        color: var(--text-faint);
        font-size: 12px;
        line-height: 1.5;
    }
    .manual :global(.actions) {
        display: flex;
        gap: 8px;
    }
    .manual :global(input[type='date']),
    .manual :global(input[type='datetime-local']),
    .manual :global(input[type='file']) {
        background: var(--bg-raised);
        border: 1px solid var(--border-strong);
        color: var(--text);
        border-radius: 5px;
        padding: 5px 8px;
        font-size: 13px;
        font-family: inherit;
    }
    .manual :global(.btn) {
        background: var(--bg-hover);
        border: 1px solid var(--border-strong);
        color: var(--text);
        border-radius: 6px;
        padding: 6px 14px;
        cursor: pointer;
        font-size: 13px;
    }
    .manual :global(.btn:hover) {
        border-color: var(--accent-dim);
    }
    .manual :global(.btn.primary) {
        background: var(--accent-dim);
        border-color: var(--accent-dim);
    }
    .manual :global(.btn.danger) {
        border-color: var(--fail);
        color: var(--fail);
    }
    .manual :global(.btn.add) {
        margin-top: 4px;
    }
    .manual :global(.btn:disabled) {
        opacity: 0.5;
        cursor: default;
    }
</style>

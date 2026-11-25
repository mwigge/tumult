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
    import { detailToForm, emptyForm, errorText, targetFilterOf } from '#lib/manual.js';
    import type { MeResponse } from '#lib/types.js';
    import ManualEntryTab from '#lib/components/ManualEntryTab.svelte';
    import ManualImport from '#lib/components/ManualImport.svelte';
    import ManualPlansTab from '#lib/components/ManualPlansTab.svelte';
    import ManualReviewTab from '#lib/components/ManualReviewTab.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import TabPanel from '#lib/components/TabPanel.svelte';
    import Tabs from '#lib/components/Tabs.svelte';
    import { withParam } from '#lib/tabs.js';
    import State from '#lib/components/State.svelte';

    const TABS = ['entry', 'plans', 'queue', 'records', 'import'] as const;
    const TAB_LABEL: Record<string, string> = {
        plans: 'Plans',
        queue: 'Verification queue',
        records: 'All records',
        import: 'Import',
    };

    const tab = $derived(page.url.searchParams.get('tab') ?? 'entry');
    // The target of the URL filters the records and the plans; the Star Map
    // drawer links here with the target of its system.
    const filter = $derived(targetFilterOf(page.url.searchParams));
    const filtered = $derived(filter.target !== '' || filter.environment !== '');
    const listsTab = $derived(tab === 'plans' || tab === 'queue' || tab === 'records');
    const targetLine = $derived(
        filter.environment
            ? `${filter.target || 'every target'} in ${filter.environment}`
            : filter.target,
    );

    let actor = $state('');
    let me = $state<MeResponse | null>(null);
    let editingId = $state<string | null>(null);
    const tabs = $derived(
        TABS.map((t) => ({
            value: t,
            label: t === 'entry' ? (editingId ? 'Draft editor' : 'New entry') : TAB_LABEL[t],
        })),
    );
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
        goto(withParam(page.url.searchParams, 'tab', t), { replace: true, reset: false });
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
    <PageHeader
        title="Manual evidence"
        sub="DR exercises and other hand-executed tests, entered under attestation"
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
            <Tabs
                id="manual"
                label="manual sections"
                options={tabs}
                value={tab}
                onchange={setTab}
            />
        </div>
    </PageHeader>

    <TabPanel id="manual" value={tab}>
        {#if filtered && listsTab}
            <p class="target" data-testid="manual-target">
                Records and plans of {targetLine}.
                <a href="?tab={tab}">Show every target</a>
            </p>
        {/if}

        {#if entryMsg && !entryMsg.ok && (tab !== 'entry' || !canWrite)}<State kind="error">
                {entryMsg.text}
            </State>{/if}

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
            {:else}<p>
                    Your role can read manual evidence. An operator can create or edit it.
                </p>{/if}
        {:else if tab === 'plans'}
            <ManualPlansTab {actor} {canWrite} {filter} />
        {:else if tab === 'import'}
            <ManualImport {actor} {canWrite} />
        {:else}
            <ManualReviewTab
                {tab}
                {actor}
                {canReview}
                canEdit={canWrite}
                {filter}
                onedit={editDraft}
            />
        {/if}
    </TabPanel>
</div>

<style>
    .controls {
        margin-left: auto;
        display: flex;
        gap: var(--space-3);
        align-items: center;
    }
    .target {
        color: var(--text-dim);
        font-size: var(--font-size-sm);
        margin: 0 0 var(--space-2-5);
    }
    .actor {
        display: flex;
        align-items: center;
        gap: var(--space-1-5);
        color: var(--text-dim);
        font-size: var(--font-size-xs);
    }
</style>

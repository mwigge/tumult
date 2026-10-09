<script lang="ts">
    // Manual evidence: enter hand-executed test records under attestation,
    // and verify/reject submitted records (reviewer ≠ enterer — enforced by
    // the API). Authenticated attribution comes from the session.
    import { page } from '$app/state';
    import { goto } from '$app/navigation';
    import { onMount } from 'svelte';
    import { api } from '#lib/api.js';
    import type { MeResponse } from '#lib/types.js';
    import ManualEntryTab, { emptyForm } from '#lib/components/ManualEntryTab.svelte';
    import ManualReviewTab from '#lib/components/ManualReviewTab.svelte';

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

    // Review notes, keyed by record id; kept here for the same reason.
    let notes: Record<string, string> = $state({});

    async function editDraft(id: string) {
        try {
            const { experiment: rec } = await api.manualDetail(id);
            if (rec.status !== 'draft') throw new Error('Only drafts can be edited.');
            const localDate = (ns: number | null) => {
                if (!ns) return '';
                const date = new Date(ns / 1e6);
                return new Date(date.getTime() - date.getTimezoneOffset() * 60000)
                    .toISOString()
                    .slice(0, 16);
            };
            form = {
                ...emptyForm(),
                experiment_name: rec.experiment_name,
                exercise_type: rec.exercise_type,
                executed: localDate(rec.executed_at_ns),
                hypothesis: rec.hypothesis,
                method: rec.method,
                outcome_status: rec.outcome_status,
                hypothesis_met:
                    rec.hypothesis_met === null
                        ? 'unknown'
                        : rec.hypothesis_met
                          ? 'met'
                          : 'not-met',
                findings: rec.findings ?? '',
                action_items: (Array.isArray(rec.action_items) ? rec.action_items : []).join('\n'),
                target_system: rec.target_system ?? '',
                target_environment: rec.target_environment ?? '',
                blast_radius: rec.blast_radius ?? '',
                recovery_time_s: rec.recovery_time_s == null ? '' : String(rec.recovery_time_s),
                duration_s: rec.duration_s == null ? '' : String(rec.duration_s),
                framework_refs: (rec.framework_refs ?? []).join(', '),
                renewal: localDate(rec.renewal_due_ns).slice(0, 10),
                attestation: rec.attestation,
            };
            editingId = id;
            entryMsg = null;
            setTab('entry');
        } catch (e) {
            entryMsg = { ok: false, text: String(e) };
        }
    }
</script>

<div class="page-head">
    <h1>Manual evidence</h1>
    <span class="sub">hand-executed tests, entered under attestation</span>
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
            {#each ['entry', 'queue', 'records'] as t (t)}
                <button class:active={tab === t} onclick={() => setTab(t)}>
                    {t === 'entry'
                        ? editingId
                            ? 'Draft editor'
                            : 'New entry'
                        : t === 'queue'
                          ? 'Verification queue'
                          : 'All records'}
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
{:else}
    <ManualReviewTab {tab} {actor} {canReview} canEdit={canWrite} onedit={editDraft} bind:notes />
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

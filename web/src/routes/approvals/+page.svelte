<script lang="ts">
    import { onMount } from 'svelte';
    import { api, fmtAgo, fmtTs, shortId } from '#lib/api.js';
    import { paramsText } from '#lib/run-params.js';
    import type { ApprovalQueueRow, MeResponse } from '#lib/types.js';
    import Badge from '#lib/components/Badge.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';
    import { onRow } from '#lib/rows.js';

    let rows = $state<ApprovalQueueRow[] | null>(null);
    let error = $state<string | null>(null);
    let me = $state<MeResponse | null>(null);

    let openId = $state<string | null>(null);
    let notes: Record<string, string> = $state({});
    // Per-row action feedback: errors stick until the next attempt, the quorum
    // hint is transient (cleared on the next refresh that drops the row).
    let rowErrors: Record<string, string> = $state({});
    let rowHints: Record<string, string> = $state({});
    let busy = $state(false);

    // Approvers and admins may decide; the server enforces it regardless —
    // this only hides buttons the role cannot use.
    const canDecide = $derived(
        me
            ? !me.auth_required ||
                  (me.authenticated && (me.role === 'approver' || me.role === 'admin'))
            : false,
    );

    onMount(() => {
        api.me()
            .then((m) => (me = m))
            .catch(() => (me = null));
    });

    // Poll the queue every 5s for as long as the page is open; the cleanup
    // clears the pending tick, so no timers leak.
    $effect(() => {
        let cancelled = false;
        let timer: ReturnType<typeof setTimeout> | null = null;

        async function poll() {
            try {
                const r = await api.approvals();
                if (cancelled) return;
                rows = r.queue;
                error = null;
            } catch (e) {
                if (cancelled) return;
                // Keep the last snapshot on transient failures once loaded.
                if (!rows) error = String(e);
            }
            if (!cancelled) timer = setTimeout(poll, 5000);
        }

        void poll();
        return () => {
            cancelled = true;
            if (timer) clearTimeout(timer);
        };
    });

    async function refresh() {
        try {
            const r = await api.approvals();
            rows = r.queue;
            error = null;
        } catch (e) {
            if (!rows) error = String(e);
        }
    }

    async function decide(runId: string, approve: boolean) {
        if (busy) return;
        busy = true;
        const errs = { ...rowErrors };
        delete errs[runId];
        rowErrors = errs;
        try {
            const note = notes[runId]?.trim() || undefined;
            const r = approve
                ? await api.approveRun(runId, note)
                : await api.rejectRun(runId, note);
            if (approve && r.state === 'pending_approval') {
                rowHints = { ...rowHints, [runId]: 'recorded — waiting for quorum' };
            } else {
                const hints = { ...rowHints };
                delete hints[runId];
                rowHints = hints;
            }
            await refresh();
        } catch (e) {
            rowErrors = { ...rowErrors, [runId]: String(e) };
        } finally {
            busy = false;
        }
    }

    function toggle(runId: string) {
        openId = openId === runId ? null : runId;
    }

    /** Remaining TTL; `soon` (under 1h left, or lapsed) gets the warn tone, and says so. */
    function remaining(ns: number): { text: string; soon: boolean } {
        const ms = ns / 1_000_000 - Date.now();
        if (ms <= 0) return { text: 'expired', soon: true };
        const m = Math.floor(ms / 60_000);
        if (m >= 60) return { text: `${Math.floor(m / 60)}h ${m % 60}m`, soon: false };
        return { text: `${m}m ${Math.floor((ms % 60_000) / 1000)}s, soon`, soon: true };
    }
</script>

<PageHeader
    title="Approvals"
    sub={rows ? `${rows.length} pending` : 'gated runs waiting on quorum'}
/>

<Panel scroll>
    {#if error}
        <State kind="error">Failed to load the approvals queue: {error}</State>
    {:else if !rows}
        <State kind="loading" height={200} />
    {:else if rows.length === 0}
        <State kind="empty">Nothing awaiting approval.</State>
    {:else}
        <table class="data">
            <thead>
                <tr>
                    <th></th>
                    <th>Definition</th>
                    <th>Run</th>
                    <th>Tier</th>
                    <th>Env</th>
                    <th>Requested</th>
                    <th>Expires</th>
                    <th>Quorum</th>
                    {#if canDecide}
                        <th>Note</th>
                        <th></th>
                    {/if}
                </tr>
            </thead>
            <tbody>
                {#each rows as row (row.run_id)}
                    {@const ttl = remaining(row.expires_at_ns)}
                    <tr class="clickable" onclick={onRow(() => toggle(row.run_id))}>
                        <td style="color: var(--text-faint)">{openId === row.run_id ? '▾' : '▸'}</td
                        >
                        <td>
                            <button
                                class="row-button"
                                aria-expanded={openId === row.run_id}
                                onclick={() => toggle(row.run_id)}
                                >{row.definition_name ?? '—'}</button
                            >
                            {#if row.break_glass}
                                <Badge
                                    tone="warn"
                                    title="break-glass override by {row.break_glass_by ??
                                        '?'}{row.break_glass_justification
                                        ? `: ${row.break_glass_justification}`
                                        : ''}"
                                    label="break-glass"
                                />
                            {/if}
                        </td>
                        <td class="mono" style="color: var(--text-dim)">
                            <a href="/runs/{row.run_id}" onclick={(e) => e.stopPropagation()}>
                                {shortId(row.run_id)}
                            </a>
                        </td>
                        <td><Badge kind="tier" value={row.tier} /></td>
                        <td>{row.env}</td>
                        <td title={fmtTs(row.requested_at_ns)}>
                            {row.requested_by} · {fmtAgo(row.requested_at_ns)}
                        </td>
                        <td
                            class="mono"
                            class:warn-text={ttl.soon}
                            title={fmtTs(row.expires_at_ns)}
                        >
                            {ttl.text}
                        </td>
                        <td class="mono">{row.approved_count}/{row.quorum_required}</td>
                        {#if canDecide}
                            <td onclick={(e) => e.stopPropagation()}>
                                <input
                                    type="text"
                                    aria-label="Decision note"
                                    placeholder="decision note…"
                                    bind:value={notes[row.run_id]}
                                />
                            </td>
                            <td class="actions" onclick={(e) => e.stopPropagation()}>
                                <button
                                    class="btn"
                                    onclick={() => decide(row.run_id, true)}
                                    disabled={busy}
                                >
                                    Approve
                                </button>
                                <button
                                    class="btn danger"
                                    onclick={() => decide(row.run_id, false)}
                                    disabled={busy}
                                >
                                    Reject
                                </button>
                            </td>
                        {/if}
                    </tr>
                    {#if rowErrors[row.run_id] || rowHints[row.run_id]}
                        <tr class="msg-row">
                            <td colspan={canDecide ? 10 : 8}>
                                {#if rowErrors[row.run_id]}
                                    <span class="err-text">{rowErrors[row.run_id]}</span>
                                {/if}
                                {#if rowHints[row.run_id]}
                                    <span class="hint-text">{rowHints[row.run_id]}</span>
                                {/if}
                            </td>
                        </tr>
                    {/if}
                    {#if openId === row.run_id}
                        <tr class="detail-row">
                            <td colspan={canDecide ? 10 : 8}>
                                <div class="detail">
                                    <div>
                                        <h2>Parameters</h2>
                                        <pre class="mono params">{paramsText(row.params_json)}</pre>
                                    </div>
                                    <div>
                                        <h2>Pin hash</h2>
                                        <div class="mono dim hash">{row.pin_hash}</div>
                                        {#if row.target}
                                            <h2>Target</h2>
                                            <div class="dim">{row.target}</div>
                                        {/if}
                                    </div>
                                </div>
                            </td>
                        </tr>
                    {/if}
                {/each}
            </tbody>
        </table>
    {/if}
</Panel>

<style>
    .warn-text {
        color: var(--warn);
    }
    .msg-row td {
        background: var(--bg-raised);
        padding: var(--space-1) var(--space-3);
        display: flex;
        gap: var(--space-3);
    }
    .err-text {
        color: var(--fail);
        font-size: var(--font-size-xs);
    }
    .hint-text {
        color: var(--ok);
        font-size: var(--font-size-xs);
    }
    .detail {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: var(--space-4);
        padding: var(--space-2) var(--space-1);
    }
    .detail h2 {
        font-size: var(--font-size-xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-sm);
        color: var(--text-dim);
        margin: 0 0 var(--space-2);
    }
    .params {
        margin: 0;
        font-size: var(--font-size-xs);
        white-space: pre-wrap;
        word-break: break-word;
        color: var(--text-dim);
    }
    .dim {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
    }
    .hash {
        word-break: break-all;
    }
</style>

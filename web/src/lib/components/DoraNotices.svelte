<script lang="ts">
    // What a score document was computed with and from: a profile other
    // than the registered one, a partial view, the house parameters in
    // force, the state of the function register and the feeds. A lowered
    // gate, a refused org file, a day that is not stored and days stored
    // by the proof tool are never silent.
    import { proofDaysText, snapshotSkippedText } from '#lib/dora.js';
    import type { ScoreHeader } from '#lib/types.js';
    import Badge from './Badge.svelte';

    let { doc }: { doc: ScoreHeader } = $props();

    const REGISTER_TEXT: Record<string, string> = {
        none: 'no org file: no function register',
        ok: 'the lake holds the register of the org file',
        stale: 'the org file changed and is not in the lake yet',
        invalid: 'the org file was refused',
    };

    const shown = (value: unknown) => (typeof value === 'string' ? value : JSON.stringify(value));

    const skipped = $derived(snapshotSkippedText(doc.snapshot.skipped));
    const proof = $derived(proofDaysText(doc.proof_days));
</script>

{#if doc.proof_days.enabled}
    <div class="notice warn" role="status" data-testid="proof-days">
        {doc.proof_days.note ?? 'Proof days are in use.'}
        <span class="dim">{proof}</span>
    </div>
{/if}
{#if doc.snapshot.skipping}
    <div class="notice warn" role="status" data-testid="snapshot-skipping">
        {doc.snapshot.reason ?? 'The daily snapshot is not stored.'}
        {#if skipped}<span class="dim">{skipped}</span>{/if}
    </div>
{/if}

{#if doc.profile_overridden}
    <div class="notice warn" role="status" data-testid="profile-override">
        Scored as profile <b>{doc.profile_label}</b> because the request asked for it. The
        registered profile is {doc.registered_profile ?? 'not on record'}; stored history and the
        daily snapshot are of the registered profile.
    </div>
{/if}
{#if doc.scoped && doc.scope_note}
    <div class="notice warn" role="status" data-testid="scope-note">{doc.scope_note}</div>
{/if}

<dl class="facts">
    <dt>House parameters</dt>
    <dd data-testid="score-params">
        {#if doc.params.fallback}
            <Badge tone="fail" label="defaults in force" />
            the <span class="mono">score:</span> section of the org file was refused: {doc.params
                .reason ?? 'no reason given'}
        {:else}
            {#if doc.params.held}
                <Badge tone="warn" label="last valid file in force" />
                the org file on disk was refused: {doc.params.reason ?? 'no reason given'}
            {/if}
            {#if doc.params.overridden.length === 0}
                house defaults, nothing overridden
            {:else}
                <Badge tone="warn" label="{doc.params.overridden.length} overridden" />
                {#each doc.params.overridden as o (o.key)}
                    <span class="override">
                        <span class="mono">{o.key}</span> =
                        <span class="mono">{shown(o.value)}</span>
                        <span class="dim">(default {shown(o.default)})</span>
                    </span>
                {/each}
            {/if}
        {/if}
    </dd>
    {#if !doc.proof_days.enabled && proof}
        <dt>Proof days</dt>
        <dd data-testid="proof-days-ignored">{proof}</dd>
    {/if}
    <dt>Function register</dt>
    <dd data-testid="register-state">
        <Badge kind="register" value={doc.register_state} />
        {doc.register_reason ?? REGISTER_TEXT[doc.register_state] ?? ''}
    </dd>
    <dt>Feeds</dt>
    <dd data-testid="score-feeds">
        {#each doc.feeds as feed (feed.name)}
            <span class="feed">
                <span class="mono">{feed.name}</span>
                <Badge kind="switch" value={feed.connected ? 'connected' : 'not connected'} />
                {#if !feed.connected && feed.reason}<span class="dim">{feed.reason}</span>{/if}
            </span>
        {:else}
            none on record
        {/each}
    </dd>
</dl>

<style>
    .notice {
        margin-top: var(--space-3);
        padding: var(--space-2) var(--space-3);
        border: var(--border-width-sm) solid var(--border-strong);
        border-left: var(--border-width-lg) solid var(--warn);
        border-radius: var(--radius-md);
        font-size: var(--font-size-xs);
    }
    .facts {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: var(--space-1) var(--space-4);
        margin: var(--space-3) 0 0;
        font-size: var(--font-size-xs);
    }
    .facts dt {
        color: var(--text-dim);
    }
    .facts dd {
        margin: 0;
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-1) var(--space-3);
        align-items: baseline;
    }
</style>

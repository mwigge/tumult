<script lang="ts">
    // What a score document was computed with and from: a profile other
    // than the registered one, a partial view, the house parameters in
    // force, the state of the function register and the feeds. A lowered
    // gate or a refused org file is never silent.
    import type { ScoreHeader } from '#lib/types.js';

    let { doc }: { doc: ScoreHeader } = $props();

    const REGISTER_TEXT: Record<string, string> = {
        none: 'no org file: no function register',
        ok: 'the lake holds the register of the org file',
        stale: 'the org file changed and is not in the lake yet',
        invalid: 'the org file was refused',
    };
    const REGISTER_CLASS: Record<string, string> = {
        none: 'neutral',
        ok: 'ok',
        stale: 'warn',
        invalid: 'fail',
    };

    const shown = (value: unknown) => (typeof value === 'string' ? value : JSON.stringify(value));
</script>

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
            <span class="badge fail">defaults in force</span>
            the <span class="mono">score:</span> section of the org file was refused: {doc.params
                .reason ?? 'no reason given'}
        {:else if doc.params.overridden.length === 0}
            house defaults, nothing overridden
        {:else}
            <span class="badge warn">{doc.params.overridden.length} overridden</span>
            {#each doc.params.overridden as o (o.key)}
                <span class="override">
                    <span class="mono">{o.key}</span> = <span class="mono">{shown(o.value)}</span>
                    <span class="dim">(default {shown(o.default)})</span>
                </span>
            {/each}
        {/if}
    </dd>
    <dt>Function register</dt>
    <dd data-testid="register-state">
        <span class="badge {REGISTER_CLASS[doc.register_state] ?? 'neutral'}"
            >{doc.register_state}</span
        >
        {doc.register_reason ?? REGISTER_TEXT[doc.register_state] ?? ''}
    </dd>
    <dt>Feeds</dt>
    <dd data-testid="score-feeds">
        {#each doc.feeds as feed (feed.name)}
            <span class="feed">
                <span class="mono">{feed.name}</span>
                <span class="badge {feed.connected ? 'ok' : 'neutral'}"
                    >{feed.connected ? 'connected' : 'not connected'}</span
                >
                {#if !feed.connected && feed.reason}<span class="dim">{feed.reason}</span>{/if}
            </span>
        {:else}
            none on record
        {/each}
    </dd>
</dl>

<style>
    .notice {
        margin-top: 10px;
        padding: 6px 10px;
        border: 1px solid var(--border-strong);
        border-left: 3px solid var(--warn);
        border-radius: 4px;
        font-size: 12.5px;
    }
    .facts {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 4px 16px;
        margin: 10px 0 0;
        font-size: 12.5px;
    }
    .facts dt {
        color: var(--text-dim);
    }
    .facts dd {
        margin: 0;
        display: flex;
        flex-wrap: wrap;
        gap: 4px 12px;
        align-items: baseline;
    }
    .dim {
        color: var(--text-faint);
        font-size: 11.5px;
    }
</style>

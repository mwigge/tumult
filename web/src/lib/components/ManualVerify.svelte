<script lang="ts">
    // The audit verify of a record or a plan: the API checks the seal, the
    // audit trail, the sign-offs and the files again, straight from the
    // lake, and says ok or lists each problem.
    import { errorText } from '#lib/manual.js';
    import type { ManualVerdict } from '#lib/types.js';

    let {
        run,
        verdict = $bindable(null),
    }: {
        run: () => Promise<ManualVerdict>;
        verdict?: ManualVerdict | null;
    } = $props();

    let busy = $state(false);
    let error: string | null = $state(null);

    async function verify() {
        busy = true;
        error = null;
        try {
            verdict = await run();
        } catch (e) {
            verdict = null;
            error = errorText(e);
        } finally {
            busy = false;
        }
    }

    const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;
</script>

<div class="verify">
    <button class="btn" disabled={busy} onclick={verify}>Verify audit trail</button>
    {#if verdict}
        <div class="verdict" class:bad={!verdict.ok} role="status" data-testid="audit-verdict">
            <p>
                {verdict.ok
                    ? 'Audit trail ok: no problem found.'
                    : `Audit trail not ok: ${plural(verdict.problems.length, 'problem')}.`}
                <span class="dim">
                    {#if verdict.hash_version}seal form {verdict.hash_version}{/if}
                    {#if verdict.legacy_rows}· {plural(verdict.legacy_rows, 'legacy row')} before the
                        link hash{/if}
                </span>
            </p>
            {#if verdict.problems.length > 0}
                <ul class="problems">
                    {#each verdict.problems as p, i (i)}
                        <li>
                            <code>{p.table}</code> row <code>{p.row}</code>
                            {#if p.field}field <code>{p.field}</code>{/if}: {p.problem}
                        </li>
                    {/each}
                </ul>
            {/if}
        </div>
    {/if}
    {#if error}<div class="state error" role="status">{error}</div>{/if}
</div>

<style>
    .verdict {
        margin-top: var(--space-2);
        padding: var(--space-2) var(--space-2-5);
        border-radius: var(--radius-xl);
        border: var(--border-width-sm) solid var(--ok);
        background: color-mix(in srgb, var(--ok) 8%, transparent);
    }
    .verdict.bad {
        border-color: var(--fail);
        background: color-mix(in srgb, var(--fail) 8%, transparent);
    }
    .verdict p {
        margin: 0 0 var(--space-1);
    }
    .problems {
        margin: 0;
        padding-left: var(--space-4-5);
    }
    .dim {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
    }
</style>

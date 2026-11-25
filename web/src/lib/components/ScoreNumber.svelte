<script lang="ts">
    // A number of the score view. `unknown` is required: no number is shown
    // without the share of it that rests on missing evidence.
    import { unknownLabel } from '#lib/dora.js';

    let {
        label,
        value,
        unknown,
        hint = '',
        muted = false,
    }: {
        label: string;
        value: string;
        /** Share (0–1) of the inputs that are unknown. */
        unknown: number;
        hint?: string;
        /** Grey the value: it is shown but does not decide the outcome. */
        muted?: boolean;
    } = $props();
</script>

<div class="panel num" data-testid="score-number">
    <div class="label">{label}</div>
    <div class="row">
        <span class="value mono" class:muted>{value}</span>
        <span class="unknown" class:some={unknown > 0} data-testid="unknown-share"
            >{unknownLabel(unknown)}</span
        >
    </div>
    {#if hint}<div class="hint">{hint}</div>{/if}
</div>

<style>
    .num {
        min-width: 0;
    }
    .label {
        color: var(--text-dim);
        font-size: var(--font-size-2xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-md);
        margin-bottom: var(--space-1-5);
    }
    .row {
        display: flex;
        align-items: baseline;
        gap: var(--space-2-5);
        flex-wrap: wrap;
    }
    .value {
        font-size: var(--font-size-3xl);
        font-weight: var(--font-weight-strong);
    }
    .value.muted {
        color: var(--text-faint);
    }
    .unknown {
        font-size: var(--font-size-2xs);
        color: var(--text-faint);
        border: var(--border-width-sm) dashed var(--border-strong);
        border-radius: var(--radius-md);
        padding: 1px var(--space-1-5);
        white-space: nowrap;
    }
    .unknown.some {
        color: var(--warn);
        border-color: var(--warn);
    }
    .hint {
        color: var(--text-faint);
        font-size: var(--font-size-2xs);
        margin-top: var(--space-1);
    }
</style>

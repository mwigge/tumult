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
        font-size: 11.5px;
        text-transform: uppercase;
        letter-spacing: 0.6px;
        margin-bottom: 6px;
    }
    .row {
        display: flex;
        align-items: baseline;
        gap: 10px;
        flex-wrap: wrap;
    }
    .value {
        font-size: 24px;
        font-weight: 600;
    }
    .value.muted {
        color: var(--text-faint);
    }
    .unknown {
        font-size: 11.5px;
        color: var(--text-faint);
        border: 1px dashed var(--border-strong);
        border-radius: 4px;
        padding: 1px 6px;
        white-space: nowrap;
    }
    .unknown.some {
        color: var(--warn);
        border-color: var(--warn);
    }
    .hint {
        color: var(--text-faint);
        font-size: 11.5px;
        margin-top: 4px;
    }
</style>

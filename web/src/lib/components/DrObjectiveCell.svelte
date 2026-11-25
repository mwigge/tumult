<script lang="ts">
    // What a DR test measured of its RTO or RPO, over the target it is held
    // to. `alarm` marks a number that missed its target, in red and in
    // words. With no target, the alarm is a test that did not recover, and
    // the number says so.
    import { seconds } from '#lib/dr.js';

    let {
        measured,
        target,
        alarm = false,
    }: { measured: string; target: number | null; alarm?: boolean } = $props();
</script>

<span class="measured" class:alarm>{measured}</span>
<span class="target"
    >{target === null ? 'no target' : `target ${seconds(target)}${alarm ? ', missed' : ''}`}</span
>

<style>
    .measured,
    .target {
        display: block;
        white-space: nowrap;
    }
    .measured.alarm {
        color: var(--fail);
    }
    .target {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
    }
</style>

<script lang="ts" generics="T extends string">
    // A group of buttons where one is chosen, such as a time range or a
    // filter. Each button says whether it is pressed. The page keeps the
    // value and hears of a choice through `onchange`. `class` is the look of
    // the group in theme.css; `selectedClass` marks the chosen button. A
    // space stands between the buttons, as a look of inline buttons shows it.
    import { choiceOf } from '#lib/choice.js';
    import type { Choice } from '#lib/choice.js';

    let {
        options,
        value,
        label,
        onchange,
        class: look = 'seg',
        selectedClass = 'active',
    }: {
        options: readonly Choice<T>[];
        value: string;
        /** The name of the group. */
        label: string;
        onchange: (value: T) => void;
        class?: string;
        selectedClass?: string;
    } = $props();

    const choices = $derived(options.map(choiceOf));
</script>

<div class={look} role="group" aria-label={label}>
    {#each choices as choice, i (choice.value)}
        {i > 0 ? ' ' : ''}<button
            aria-pressed={choice.value === value}
            class={choice.value === value ? selectedClass : undefined}
            onclick={() => onchange(choice.value)}>{choice.label}</button
        >
    {/each}
</div>

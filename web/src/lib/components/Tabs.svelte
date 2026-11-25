<script lang="ts" generics="T extends string">
    // Tabs that switch the panel below them, after the WAI-ARIA tabs pattern:
    // the arrow keys, Home and End move to a tab and select it, and only the
    // chosen tab is in the tab order. The page keeps the value in its URL,
    // hears of a choice through `onchange` and draws the panel with
    // TabPanel.svelte of the same `id`. `class` and `selectedClass` are those
    // of Segmented.svelte.
    import { choiceOf } from '#lib/choice.js';
    import type { Choice } from '#lib/choice.js';
    import { panelId, tabAfterKey, tabId } from '#lib/tabs.js';

    let {
        id,
        options,
        value,
        label,
        onchange,
        class: look = 'seg',
        selectedClass = 'active',
    }: {
        id: string;
        options: readonly Choice<T>[];
        value: string;
        /** The name of the tab list. */
        label: string;
        onchange: (value: T) => void;
        class?: string;
        selectedClass?: string;
    } = $props();

    const choices = $derived(options.map(choiceOf));
    // The tab in the tab order: the chosen one, or the first when none is.
    const reachable = $derived(
        Math.max(
            0,
            choices.findIndex((c) => c.value === value),
        ),
    );
    let list: HTMLDivElement | undefined = $state();

    function move(event: KeyboardEvent, index: number) {
        const next = tabAfterKey(event.key, index, choices.length);
        if (next === null) return;
        event.preventDefault();
        list?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
        onchange(choices[next].value);
    }
</script>

<div class={look} role="tablist" aria-label={label} bind:this={list}>
    {#each choices as choice, i (choice.value)}
        {i > 0 ? ' ' : ''}<button
            role="tab"
            id={tabId(id, choice.value)}
            aria-selected={choice.value === value}
            aria-controls={panelId(id)}
            tabindex={i === reachable ? 0 : -1}
            class={choice.value === value ? selectedClass : undefined}
            onclick={() => onchange(choice.value)}
            onkeydown={(event) => move(event, i)}>{choice.label}</button
        >
    {/each}
</div>

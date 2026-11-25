<script lang="ts">
    // A panel of a page: the box that holds one part, with its title in an
    // h2. The `meta` variant is a stat tile: the title is the small capital
    // name above the value. The rules are `.panel` in theme.css.
    import type { Snippet } from 'svelte';
    import type { HTMLAttributes } from 'svelte/elements';

    let {
        title,
        variant,
        tag = 'div',
        children,
        ...attributes
    }: {
        title?: string;
        variant?: 'meta';
        /** A `section` for a panel that a heading names. */
        tag?: 'div' | 'section';
        children?: Snippet;
    } & Omit<HTMLAttributes<HTMLElement>, 'class' | 'title'> = $props();
</script>

<svelte:element this={tag} {...attributes} class={variant ? `panel ${variant}` : 'panel'}>
    {#if title !== undefined}
        {#if variant === 'meta'}<span>{title}</span>{:else}<h2>{title}</h2>{/if}
    {/if}
    {@render children?.()}
</svelte:element>

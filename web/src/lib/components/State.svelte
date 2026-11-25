<script lang="ts">
    // A part of a page that has no data to show: nothing there (`empty`), a
    // failure (`error`) or data on its way (`loading`). An error is an alert,
    // so a screen reader says it at once. A loading block is busy and holds
    // a hidden word for a screen reader. The rules are `.state` and
    // `.skeleton` in theme.css.
    import type { Snippet } from 'svelte';
    import type { HTMLAttributes } from 'svelte/elements';

    let {
        kind,
        action,
        height,
        label = 'Loading',
        panel = false,
        tag = 'div',
        style,
        children,
        ...attributes
    }: {
        kind: 'empty' | 'error' | 'loading';
        /** A link after the message, to what the reader can do next. */
        action?: { href: string; label: string };
        /** The height in px of a loading block. With none, a loading state is its message. */
        height?: number;
        /** The hidden word of a loading block for a screen reader. */
        label?: string;
        /** Draw the state in a panel box. */
        panel?: boolean;
        /** A `span` for a state in a line of text. */
        tag?: 'div' | 'span';
        style?: string;
        children?: Snippet;
    } & Omit<HTMLAttributes<HTMLElement>, 'class' | 'style'> = $props();

    // The space between the message and its link: Svelte trims a space
    // that starts a block.
    const GAP = ' ';
    const block = $derived(kind === 'loading' && height !== undefined);
    const classes = $derived(
        [block ? 'skeleton' : 'state', kind === 'error' && 'error', panel && 'panel']
            .filter(Boolean)
            .join(' '),
    );
    const styles = $derived(
        block ? [`height: ${height}px`, style].filter(Boolean).join('; ') : style,
    );
</script>

<svelte:element
    this={tag}
    {...attributes}
    class={classes}
    style={styles}
    role={kind === 'error' ? 'alert' : attributes.role}
    aria-busy={kind === 'loading' ? 'true' : attributes['aria-busy']}
>
    {#if block}
        <span class="sr-only">{label}</span>
    {:else}
        {@render children?.()}{#if action}{GAP}<a href={action.href}>{action.label}</a>.{/if}
    {/if}
</svelte:element>

<script lang="ts">
    // A drawer on a native <dialog>, named by its heading. A modal drawer
    // lies over the right edge of the window, in the top layer, and the
    // page behind it cannot be used meanwhile. A drawer that is not modal
    // is a panel in the flow of the page, beside what it describes. Escape
    // closes either, and the focus goes back to the element that opened
    // it. See drawer.ts for how it behaves.
    import type { Snippet } from 'svelte';
    import type { HTMLDialogAttributes } from 'svelte/elements';

    import { drawer } from '#lib/drawer.js';

    type Props = Omit<HTMLDialogAttributes, 'children' | 'class' | 'open'> & {
        /** The id of the heading that names the drawer. */
        labelledby: string;
        /** Close the drawer: the caller removes it. */
        onclose: () => void;
        /** False for a panel in the flow of the page. */
        modal?: boolean;
        /** The width of a modal drawer. */
        width?: 'narrow' | 'wide';
        /** Shade the page behind a modal drawer; a click on the shade closes it. */
        backdrop?: boolean;
        /** What takes the focus on open: the first control, as the browser picks, or the heading. */
        focus?: 'first' | 'heading';
        children: Snippet;
    };

    let {
        labelledby,
        onclose,
        modal = true,
        width = 'narrow',
        backdrop = false,
        focus = 'first',
        children,
        ...rest
    }: Props = $props();
</script>

<dialog
    class={['drawer', modal ? width : 'inline', { shade: modal && backdrop }]}
    aria-labelledby={labelledby}
    {...rest}
    {@attach drawer({
        modal,
        backdrop: modal && backdrop,
        focus: focus === 'heading' ? labelledby : null,
        close: () => onclose(),
    })}
>
    {@render children()}
</dialog>

<style>
    /* The browser draws a dialog as a box in the middle of the window:
       its margin, border, padding and colours go. */
    .drawer {
        margin: 0;
        border: none;
        color: inherit;
        background: var(--bg-raised);
        overflow-y: auto;
    }
    .narrow,
    .wide {
        position: fixed;
        inset: 0 0 0 auto;
        height: auto;
        max-width: none;
        max-height: none;
        border-left: var(--border-width-sm) solid var(--border-strong);
    }
    .narrow {
        width: min(var(--drawer-narrow), 92vw);
        padding: var(--space-5);
    }
    /* The wide drawer leaves the page unshaded, so a shadow marks its edge. */
    .wide {
        width: min(var(--drawer-wide), 100vw);
        padding: var(--space-4) var(--space-5);
        box-shadow: -8px 0 24px rgb(0 0 0 / 25%);
    }
    .drawer::backdrop {
        background: transparent;
    }
    .shade::backdrop {
        background: rgba(0, 0, 0, 0.45);
    }
    /* Beside the Star Map, as wide as its flex basis. */
    .inline {
        position: static;
        flex: 0 0 360px;
        width: auto;
        height: auto;
        max-width: 100%;
        max-height: 640px;
        border: var(--border-width-sm) solid var(--border-strong);
        border-radius: var(--radius-xl);
        padding: var(--space-4);
    }

    /* The parts that the drawers share. */
    .drawer :global(.close) {
        background: transparent;
        border: none;
        color: var(--text-dim);
        font-size: var(--font-size-2xl);
        cursor: pointer;
        line-height: var(--line-height-tight);
    }
    .drawer :global(h3) {
        font-size: var(--font-size-3xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-md);
        color: var(--text-dim);
        margin-bottom: var(--space-2);
    }
    .drawer :global(dt) {
        color: var(--text-dim);
    }
</style>

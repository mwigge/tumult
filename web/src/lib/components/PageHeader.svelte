<script lang="ts">
    // The head of a page: a link back to the page above, the title, a sub
    // line, then what the page puts after them, such as badges and the
    // controls on the right. The title also names the browser tab. The
    // rules are `.page-head` in theme.css.
    import type { Snippet } from 'svelte';

    import Title from './Title.svelte';

    let {
        title,
        back,
        sub,
        mono = false,
        children,
    }: {
        title: string;
        /** The page above, shown as `← label` before the title. */
        back?: { href: string; label: string };
        /** A short line after the title. */
        sub?: string;
        /** Show the title in the mono font, as for an id. */
        mono?: boolean;
        /** Badges and controls after the title and the sub line. */
        children?: Snippet;
    } = $props();
</script>

<Title name={title} />

<div class="page-head">
    {#if back}<a href={back.href} class="back">← {back.label}</a>{/if}
    <h1 class={mono ? 'mono' : undefined}>{title}</h1>
    {#if sub !== undefined}<span class="sub">{sub}</span>{/if}
    {@render children?.()}
</div>

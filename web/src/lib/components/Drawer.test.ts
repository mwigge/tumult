import { createRawSnippet } from 'svelte';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import Drawer from './Drawer.svelte';

const children = createRawSnippet(() => ({
    render: () => '<h2 id="title-1">resilience.action</h2>',
}));

/** The markup of a drawer, without comments and the class of its scoped style. */
const html = (props: Record<string, unknown>) =>
    render(Drawer, {
        props: { labelledby: 'title-1', onclose: () => {}, children, ...props } as never,
    })
        .body.replace(/<!--[^>]*-->/g, '')
        .replace(/ svelte-[a-z0-9]+/g, '');

describe('a drawer', () => {
    it('is a dialog that its heading names, closed until it is shown', () => {
        expect(html({})).toBe(
            '<dialog class="drawer narrow" aria-labelledby="title-1"><h2 id="title-1">resilience.action</h2></dialog>',
        );
    });

    it('takes its width from a prop, and shades the page behind it when asked', () => {
        expect(html({ width: 'wide', backdrop: true })).toMatch(
            /^<dialog class="drawer wide shade" aria-labelledby="title-1">/,
        );
    });

    it('is a panel in the flow of the page when it is not modal', () => {
        expect(html({ modal: false, 'data-testid': 'starmap-drawer' })).toMatch(
            /^<dialog class="drawer inline" aria-labelledby="title-1" data-testid="starmap-drawer">/,
        );
    });
});

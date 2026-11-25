import { createRawSnippet } from 'svelte';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import MetaRow from './MetaRow.svelte';

/** The markup without the comments Svelte leaves for hydration, and no space between tags. */
const markup = (html: string) => html.replace(/<!--.*?-->/g, '').replace(/>\s+</g, '><');

const tiles = createRawSnippet(() => ({
    render: () => '<div class="panel meta"><span>Queued</span><b>12:00</b></div>',
}));

describe('a meta row', () => {
    it('lays its meta tiles in one grid, with no inline style', () => {
        const html = markup(render(MetaRow, { props: { children: tiles } }).body);

        expect(html).toMatch(
            /^<div class="grid svelte-[a-z0-9]+"><div class="panel meta"><span>Queued<\/span><b>12:00<\/b><\/div><\/div>$/,
        );
    });
});

import { createRawSnippet } from 'svelte';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import PageHeader from './PageHeader.svelte';

/** The markup without the comments Svelte leaves for hydration, and no space between or around tags. */
const markup = (html: string) =>
    html
        .replace(/<!--.*?-->/g, '')
        .replace(/>\s+</g, '><')
        .trim();

const controls = createRawSnippet(() => ({
    render: () => '<div class="controls"><button>New run</button></div>',
}));

describe('the head of a page', () => {
    it('shows the title as the heading of the page', () => {
        const html = render(PageHeader, { props: { title: 'Runs' } }).body;

        expect(markup(html)).toBe('<div class="page-head"><h1>Runs</h1></div>');
    });

    it('links back to the page above with the back class and no inline style', () => {
        const html = render(PageHeader, {
            props: { title: 'New run', back: { href: '/runs', label: 'runs' } },
        }).body;

        expect(markup(html)).toBe(
            '<div class="page-head"><a href="/runs" class="back">← runs</a><h1>New run</h1></div>',
        );
    });

    it('shows the sub line after the title', () => {
        const html = render(PageHeader, { props: { title: 'Runs', sub: 'newest first' } }).body;

        expect(markup(html)).toBe(
            '<div class="page-head"><h1>Runs</h1><span class="sub">newest first</span></div>',
        );
    });

    it('puts the controls after the title and the sub line', () => {
        const html = render(PageHeader, {
            props: { title: 'Runs', sub: '3 runs', children: controls },
        }).body;

        expect(markup(html)).toBe(
            '<div class="page-head"><h1>Runs</h1><span class="sub">3 runs</span>' +
                '<div class="controls"><button>New run</button></div></div>',
        );
    });

    it('names the browser tab after the title', () => {
        const head = render(PageHeader, { props: { title: 'Runs' } }).head;

        expect(markup(head)).toBe('<title>Runs · Krönika</title>');
    });

    it('shows an id as the title in the mono font', () => {
        const html = render(PageHeader, { props: { title: 't-1', mono: true } }).body;

        expect(markup(html)).toBe('<div class="page-head"><h1 class="mono">t-1</h1></div>');
    });
});

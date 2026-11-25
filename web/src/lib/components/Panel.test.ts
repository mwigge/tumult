import { createRawSnippet } from 'svelte';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import Panel from './Panel.svelte';

/** The markup without the comments Svelte leaves for hydration, and no space between tags. */
const markup = (html: string) => html.replace(/<!--.*?-->/g, '').replace(/>\s+</g, '><');

const body = (html: string) => createRawSnippet(() => ({ render: () => html }));

describe('a panel', () => {
    it('holds its part in a plain panel', () => {
        const html = render(Panel, { props: { children: body('<p>no runs yet</p>') } }).body;

        expect(markup(html)).toBe('<div class="panel"><p>no runs yet</p></div>');
    });

    it('names its part with a title above it', () => {
        const html = render(Panel, {
            props: { title: 'Logs (2)', children: body('<table></table>') },
        }).body;

        expect(markup(html)).toBe('<div class="panel"><h2>Logs (2)</h2><table></table></div>');
    });

    it('is a stat tile with its name above the value', () => {
        const html = render(Panel, {
            props: { variant: 'meta', title: 'Queued', children: body('<b>12:00</b>') },
        }).body;

        expect(markup(html)).toBe('<div class="panel meta"><span>Queued</span><b>12:00</b></div>');
    });

    it('lets a table wider than it scroll sideways inside it', () => {
        const html = render(Panel, {
            props: { title: 'Runs', scroll: true, children: body('<table></table>') },
        }).body;

        expect(markup(html)).toBe(
            '<div class="panel" data-scroll=""><h2>Runs</h2><table></table></div>',
        );
    });

    it('passes its attributes on to the box', () => {
        const html = render(Panel, {
            props: {
                style: 'margin-bottom: var(--space-4)',
                'data-testid': 'impact',
                children: body('<p>x</p>'),
            },
        }).body;

        expect(markup(html)).toMatch(/^<div [^>]*class="panel"[^>]*>/);
        expect(markup(html)).toContain('style="margin-bottom: var(--space-4)"');
        expect(markup(html)).toContain('data-testid="impact"');
    });

    it('can be a section named by its heading', () => {
        const html = render(Panel, {
            props: {
                tag: 'section',
                'aria-labelledby': 'perf-delta-h',
                children: body('<h2 id="perf-delta-h">Difference</h2>'),
            },
        }).body;

        expect(markup(html)).toBe(
            '<section aria-labelledby="perf-delta-h" class="panel">' +
                '<h2 id="perf-delta-h">Difference</h2></section>',
        );
    });
});

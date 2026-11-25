import { createRawSnippet } from 'svelte';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import State from './State.svelte';

/** The markup without the comments Svelte leaves for hydration, and no space between tags. */
const markup = (html: string) => html.replace(/<!--.*?-->/g, '').replace(/>\s+</g, '><');

const body = (html: string) => createRawSnippet(() => ({ render: () => html }));

describe('an empty state', () => {
    it('says that there is nothing to show', () => {
        const html = render(State, {
            props: { kind: 'empty', children: body('<span>No runs match this filter.</span>') },
        }).body;

        expect(markup(html)).toBe(
            '<div class="state"><span>No runs match this filter.</span></div>',
        );
    });

    it('ends with a link to what the reader can do next', () => {
        const html = render(State, {
            props: {
                kind: 'empty',
                action: { href: '/dr', label: 'Show every target' },
                children: body('<span>No target matches these filters.</span>'),
            },
        }).body;

        expect(html.replace(/<!--.*?-->/g, '')).toBe(
            '<div class="state"><span>No target matches these filters.</span> ' +
                '<a href="/dr">Show every target</a>.</div>',
        );
    });

    it('has no link when it is given none', () => {
        const html = render(State, {
            props: { kind: 'empty', children: body('<span>No logs.</span>') },
        }).body;

        expect(html).not.toContain('<a');
    });
});

describe('an error state', () => {
    it('is an alert, so a screen reader says it at once', () => {
        const html = render(State, {
            props: { kind: 'error', children: body('<span>Failed to load runs: boom</span>') },
        }).body;

        expect(markup(html)).toBe(
            '<div class="state error" role="alert"><span>Failed to load runs: boom</span></div>',
        );
    });

    it('stays an alert when the page asks for another role', () => {
        const html = render(State, {
            props: { kind: 'error', role: 'status', children: body('<span>Not saved</span>') },
        }).body;

        expect(html).toContain('role="alert"');
        expect(html).not.toContain('role="status"');
    });

    it('can sit in a panel box', () => {
        const html = render(State, {
            props: { kind: 'error', panel: true, children: body('<span>not found</span>') },
        }).body;

        expect(markup(html)).toBe(
            '<div class="state error panel" role="alert"><span>not found</span></div>',
        );
    });

    it('can be a span in a line of text', () => {
        const html = render(State, {
            props: { kind: 'error', tag: 'span', children: body('<b>failed</b>') },
        }).body;

        expect(markup(html)).toBe('<span class="state error" role="alert"><b>failed</b></span>');
    });
});

describe('a loading state', () => {
    it('is a busy block of a set height with a hidden word for a screen reader', () => {
        const html = render(State, { props: { kind: 'loading', height: 240 } }).body;

        expect(markup(html)).toBe(
            '<div class="skeleton" style="height: 240px" aria-busy="true">' +
                '<span class="sr-only">Loading</span></div>',
        );
    });

    it('says what is loading when it is told', () => {
        const html = render(State, {
            props: { kind: 'loading', height: 200, label: 'Loading the impact' },
        }).body;

        expect(markup(html)).toContain('<span class="sr-only">Loading the impact</span>');
    });

    it('keeps the style of the page after its height', () => {
        const html = render(State, {
            props: { kind: 'loading', height: 120, style: 'margin-bottom: var(--space-3-5)' },
        }).body;

        expect(html).toContain('style="height: 120px; margin-bottom: var(--space-3-5)"');
    });

    it('is a busy line of text when it has no height', () => {
        const html = render(State, {
            props: { kind: 'loading', children: body('<span>Loading targets…</span>') },
        }).body;

        expect(markup(html)).toBe(
            '<div class="state" aria-busy="true"><span>Loading targets…</span></div>',
        );
    });
});

describe('every state', () => {
    it('passes its attributes on to the box', () => {
        const html = render(State, {
            props: {
                kind: 'empty',
                style: 'text-align: left',
                'data-testid': 'no-trace',
                children: body('<span>no trace</span>'),
            },
        }).body;

        expect(html).toContain('style="text-align: left"');
        expect(html).toContain('data-testid="no-trace"');
        expect(html).toContain('class="state"');
    });
});

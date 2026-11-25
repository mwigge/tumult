import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

// The rules that many pages share are written once, in theme.css. A page
// uses the class and keeps no copy of the rule in its own <style> block.
// The Manual page styles its parts through these classes, not :global().

const SRC = fileURLToPath(new URL('..', import.meta.url));

/** One rule of CSS: its selectors and its declarations as `property: value`. */
interface Rule {
    selectors: string[];
    declarations: string[];
}

/**
 * A rule of theme.css that pages share. A rule of a page is a copy of it
 * when it holds every declaration of `body`, under one of the selectors
 * of `copies` (also inside another selector, as `.manual .form`), or under
 * any selector when `copies` is '*'.
 */
interface Shared {
    selector: string;
    body: string[];
    copies: string[] | '*';
}

const BUTTON = [
    'background: var(--bg-hover)',
    'border: var(--border-width-sm) solid var(--border-strong)',
    'color: var(--text)',
    'border-radius: var(--radius-xl)',
    'padding: var(--space-2) var(--space-4)',
    'cursor: pointer',
    'font-size: var(--font-size-md)',
];
const DISABLED = ['opacity: 0.5', 'cursor: default'];
const FAINT = ['color: var(--text-faint)', 'font-size: var(--font-size-xs)'];
const LABEL = [
    'display: block',
    'color: var(--text-dim)',
    'font-size: var(--font-size-3xs)',
    'text-transform: uppercase',
    'letter-spacing: var(--letter-spacing-md)',
    'margin-bottom: var(--space-1)',
];
const CHIP = [
    'background: var(--bg-raised)',
    'border: var(--border-width-sm) solid var(--border-strong)',
    'border-radius: var(--radius-pill)',
    'color: var(--text)',
    'cursor: pointer',
    'font-size: var(--font-size-xs)',
    'font-family: var(--mono, monospace)',
    'padding: var(--space-0-5) var(--space-3)',
];
const SR_ONLY = [
    'position: absolute',
    'width: 1px',
    'height: 1px',
    'overflow: hidden',
    'clip-path: inset(50%)',
    'white-space: nowrap',
];
const FIELD = [
    'background: var(--bg-raised)',
    'border: var(--border-width-sm) solid var(--border-input)',
    'color: var(--text)',
    'border-radius: var(--radius-xl)',
    'padding: var(--space-1) var(--space-2)',
    'font-size: var(--font-size-md)',
    'font-family: inherit',
];
const ROW_HEADER = [
    'color: var(--text)',
    'font-size: var(--font-size-md)',
    'text-transform: none',
    'padding: var(--space-2) var(--space-3)',
    'border-bottom: var(--border-width-sm) solid var(--border)',
];

/** The rules that pages share, as theme.css writes them. */
const SHARED: Shared[] = [
    { selector: '.btn', body: BUTTON, copies: '*' },
    { selector: '.btn:hover', body: ['border-color: var(--accent-dim)'], copies: ['.btn:hover'] },
    {
        selector: '.btn.primary',
        body: ['background: var(--accent-dim)', 'border-color: var(--accent-dim)'],
        copies: ['.btn.primary'],
    },
    {
        selector: '.btn.danger',
        body: ['border-color: var(--fail)', 'color: var(--fail)'],
        copies: ['.btn.danger'],
    },
    { selector: '.btn.add', body: ['margin-top: var(--space-1)'], copies: ['.btn.add'] },
    { selector: '.btn:disabled', body: DISABLED, copies: ['.btn:disabled', '.halt:disabled'] },
    { selector: 'button.primary:disabled', body: DISABLED, copies: ['button.primary:disabled'] },
    { selector: '.dim', body: FAINT, copies: ['.dim'] },
    { selector: '.note', body: FAINT, copies: ['.note'] },
    { selector: '.label-caps', body: LABEL, copies: '*' },
    { selector: '.panel.meta span', body: LABEL, copies: ['.meta span'] },
    {
        selector: '.panel.meta b',
        body: ['font-weight: var(--font-weight-strong)', 'font-size: var(--font-size-xl)'],
        copies: ['.meta b'],
    },
    { selector: '.chip', body: CHIP, copies: '*' },
    {
        selector: '.chip .x',
        body: ['color: var(--text-dim)', 'margin-left: var(--space-1)'],
        copies: ['.chip .x'],
    },
    { selector: '.chip:hover .x', body: ['color: var(--fail)'], copies: ['.chip:hover .x'] },
    { selector: '.sr-only', body: SR_ONLY, copies: '*' },
    {
        selector: '.detail-row > td',
        body: ['background: var(--bg-raised)'],
        copies: ['.detail-row td', '.detail-row > td'],
    },
    {
        selector: '.hint',
        body: [
            'color: var(--text-dim)',
            'font-size: var(--font-size-xs)',
            'margin-top: var(--space-2)',
        ],
        copies: ['.hint'],
    },
    { selector: '.ts', body: ['color: var(--text-faint)', 'flex: 0 0 128px'], copies: ['.ts'] },
    { selector: '.actions', body: ['display: flex', 'gap: var(--space-2)'], copies: ['.actions'] },
    {
        selector: '.form',
        body: ['display: flex', 'flex-direction: column', 'gap: var(--space-3)'],
        copies: ['.form'],
    },
    {
        selector: '.form label',
        body: [
            'display: flex',
            'flex-direction: column',
            'gap: var(--space-1)',
            'font-size: var(--font-size-xs)',
            'color: var(--text-dim)',
        ],
        copies: ['.form label'],
    },
    {
        selector: '.row2',
        body: ['display: grid', 'grid-template-columns: 1fr 1fr', 'gap: var(--space-3)'],
        copies: ['.row2'],
    },
    {
        selector: '.grid-rows',
        body: [
            'display: grid',
            'gap: var(--space-2)',
            'align-items: center',
            'margin-bottom: var(--space-2)',
            'font-size: var(--font-size-xs)',
            'color: var(--text-dim)',
        ],
        copies: ['.grid-rows'],
    },
    {
        selector: '.grid-rows input',
        body: ['min-width: 0', 'width: 100%'],
        copies: ['.grid-rows input'],
    },
    {
        selector: '.grid-rows select',
        body: ['min-width: 0', 'width: 100%'],
        copies: ['.grid-rows select'],
    },
    {
        selector: '.grid-rows .n',
        body: ['color: var(--text-faint)', 'text-align: right'],
        copies: ['.grid-rows .n'],
    },
    { selector: "input[type='date']", body: FIELD, copies: ["input[type='date']"] },
    {
        selector: "input[type='datetime-local']",
        body: FIELD,
        copies: ["input[type='datetime-local']"],
    },
    { selector: "input[type='file']", body: FIELD, copies: ["input[type='file']"] },
    { selector: "table.data th[scope='row']", body: ROW_HEADER, copies: ["th[scope='row']"] },
    {
        selector: "table.data tr:last-child th[scope='row']",
        body: ['border-bottom: none'],
        copies: ["tr:last-child th[scope='row']"],
    },
];

/** `text` split at each comma that is outside brackets. */
function splitSelectors(text: string): string[] {
    const parts: string[] = [];
    let depth = 0;
    let part = '';
    for (const char of text) {
        if ('(['.includes(char)) depth += 1;
        if (')]'.includes(char)) depth -= 1;
        if (char === ',' && depth === 0) {
            parts.push(part);
            part = '';
            continue;
        }
        part += char;
    }
    return [...parts, part].map((selector) => selector.trim().replace(/\s+/g, ' '));
}

/** A selector of a page as the browser reads it: a :global() part loses its wrapper. */
const unwrapped = (selector: string) => selector.replace(/:global\(([^()]*)\)/g, '$1');

/** The style rules of CSS text; the rules inside @media count, @keyframes steps do not. */
function rulesOf(css: string): Rule[] {
    const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
    const rules: Rule[] = [];
    for (const [, prelude, body] of text.matchAll(/([^{};]+)\{([^{}]*)\}/g)) {
        const head = prelude.trim();
        if (head.startsWith('@') || /^(?:from|to|[\d.]+%)$/.test(head)) continue;
        const declarations = body
            .split(';')
            .map((declaration) => declaration.trim().replace(/\s+/g, ' '))
            .filter((declaration) => declaration !== '');
        rules.push({ selectors: splitSelectors(head).map(unwrapped), declarations });
    }
    return rules;
}

/** CSS text without its @media blocks: the rules that hold at every width. */
const withoutMedia = (css: string) =>
    css.replace(/@media[^{]*\{(?:[^{}]*\{[^{}]*\})*[^{}]*\}/g, '');

/** The CSS of the <style> blocks of a Svelte file. */
const styleOf = (svelte: string) =>
    [...svelte.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)].map((block) => block[1]).join('\n');

/** Whether `rule` is a copy of `shared`. */
function isCopy(rule: Rule, shared: Shared): boolean {
    if (!shared.body.every((declaration) => rule.declarations.includes(declaration))) return false;
    if (shared.copies === '*') return true;
    const names = shared.copies;
    return rule.selectors.some((selector) =>
        names.some((name) => selector === name || selector.endsWith(` ${name}`)),
    );
}

/** The copies of `shared` in the <style> blocks of `svelte`, the text of `file`. */
function copiesIn(file: string, svelte: string, shared: Shared): string[] {
    return rulesOf(styleOf(svelte))
        .filter((rule) => isCopy(rule, shared))
        .map(() => file);
}

/** The Svelte files under `dir`, as paths from src/. */
function svelteFiles(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return svelteFiles(path);
        return entry.name.endsWith('.svelte') ? [relative(SRC, path)] : [];
    });
}

const read = (path: string) => readFileSync(join(SRC, path), 'utf8');
const FILES = svelteFiles(SRC).sort();

describe('the scan for copies of a shared rule', () => {
    it('finds a copy under its own selector, inside another selector and under any selector', () => {
        const page = [
            '<style>',
            '    .btn:disabled { cursor: default; opacity: 0.5; }',
            '    .manual :global(.btn:disabled) { opacity: 0.5; cursor: default; }',
            '    .x:disabled { opacity: 0.5; cursor: default; }',
            '    @media (max-width: 900px) { .y, .halt { ' + BUTTON.join('; ') + '; } }',
            '</style>',
        ].join('\n');
        const disabled = SHARED.find((rule) => rule.selector === '.btn:disabled');
        const button = SHARED.find((rule) => rule.selector === '.btn');

        expect(disabled && copiesIn('x.svelte', page, disabled)).toEqual(['x.svelte', 'x.svelte']);
        expect(button && copiesIn('x.svelte', page, button)).toEqual(['x.svelte']);
    });

    it('takes a rule with only part of the body for its own rule', () => {
        const page = '<style> .btn { white-space: nowrap; cursor: pointer; } </style>';
        const button = SHARED.find((rule) => rule.selector === '.btn');

        expect(button && copiesIn('x.svelte', page, button)).toEqual([]);
    });
});

describe('the rules that pages share', () => {
    it('are each written once in theme.css', () => {
        // A media query may change a part of a rule on a small screen: that is no second rule.
        const rules = rulesOf(withoutMedia(read('lib/theme.css')));
        const wrong = SHARED.flatMap(({ selector, body }) => {
            const found = rules.filter((rule) => rule.selectors.includes(selector));
            if (found.length !== 1) return [`${selector}: ${found.length} rules in theme.css`];
            const missing = body.filter(
                (declaration) => !found[0].declarations.includes(declaration),
            );
            return missing.length > 0
                ? [`${selector}: theme.css has no ${missing.join('; ')}`]
                : [];
        });

        expect(wrong).toEqual([]);
    });

    it('have no copy in the style block of a page or a component', () => {
        const copies = SHARED.flatMap((shared) => {
            const where = FILES.flatMap((file) => copiesIn(file, read(file), shared));
            if (where.length === 0) return [];
            return [`${shared.selector}: ${where.length} copies, in ${where.join(', ')}`];
        });

        expect(copies).toEqual([]);
    });
});

describe('the Manual page and its components', () => {
    it('style no part of another component through :global()', () => {
        const manual = FILES.filter(
            (file) =>
                file.startsWith(join('routes', 'manual')) ||
                /^Manual.*\.svelte$/.test(relative(join('lib', 'components'), file)),
        );
        const global = manual.flatMap((file) => {
            const count = read(file).split(':global(').length - 1;
            return count > 0 ? [`${file}: ${count}`] : [];
        });

        expect(manual).toContain(join('routes', 'manual', '+page.svelte'));
        expect(manual.length).toBeGreaterThan(10);
        expect(global).toEqual([]);
    });
});

import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { escapeHtml, NEUTRAL_DOT, strong, swatch, tip, tipFormatter } from './tooltip';
import type { Tip } from './tooltip';

const EVIL = `<img src=x onerror="a('b')">&`;
const SHOWN = '&lt;img src=x onerror=&quot;a(&#39;b&#39;)&quot;&gt;&amp;';

describe('escapeHtml', () => {
    it('escapes everything HTML gives meaning to', () => {
        expect(escapeHtml(EVIL)).toBe(SHOWN);
    });

    it('prints nothing for a missing value and a number as it is', () => {
        expect(escapeHtml(null)).toBe('');
        expect(escapeHtml(undefined)).toBe('');
        expect(escapeHtml(0)).toBe('0');
    });
});

describe('tip', () => {
    it('escapes a line, a piece of a line and a bold piece', () => {
        expect(tip(EVIL).html).toBe(SHOWN);
        expect(tip([EVIL, ' · ', EVIL]).html).toBe(`${SHOWN} · ${SHOWN}`);
        expect(tip([strong(EVIL), ' (x)']).html).toBe(`<b>${SHOWN}</b> (x)`);
        expect(tip(strong(EVIL)).html).toBe(`<b>${SHOWN}</b>`);
    });

    it('has no way to pass markup through', () => {
        expect(tip('<br/>', '<b>x</b>').html).toBe('&lt;br/&gt;<br/>&lt;b&gt;x&lt;/b&gt;');
        // A tip handed to a tip is text like any other value.
        const inner = tip(strong('a')) as unknown as string;
        expect(tip(inner).html).not.toContain('<b>');
        // So is something that only looks like a bold piece.
        const fake = { text: '<i>x</i>' } as unknown as string;
        expect(tip(fake).html).not.toContain('<i>');
    });

    it('joins lines with a break and leaves empty ones out', () => {
        expect(tip('a', '', null, undefined, [], ['', null], 'b').html).toBe('a<br/>b');
        expect(tip(0, [1, ' of ', 2]).html).toBe('0<br/>1 of 2');
        expect(tip().html).toBe('');
    });
});

describe('swatch', () => {
    const dot = (color: string) =>
        `<span style="display:inline-block;margin-right:4px;border-radius:50%;width:10px;height:10px;background-color:${color}"></span>`;

    it('is a dot in a plain colour before the text of its line', () => {
        expect(tip([swatch('#5470c6'), 'forecast']).html).toBe(`${dot('#5470c6')}forecast`);
        expect(tip([swatch('rgba(84, 112, 198, 0.5)')]).html).toBe(dot('rgba(84, 112, 198, 0.5)'));
        expect(tip([swatch('tomato')]).html).toBe(dot('tomato'));
    });

    it('gives a neutral dot for what is not a plain colour', () => {
        expect(NEUTRAL_DOT).toBe('#888');
        for (const color of [
            EVIL,
            'red"><img src=x>',
            'red;background:url(//x)',
            'url(javascript:a)',
            { type: 'linear' },
            null,
            undefined,
            '',
        ]) {
            expect(tip([swatch(color), 'name']).html).toBe(`${dot(NEUTRAL_DOT)}name`);
        }
    });
});

describe('tipFormatter', () => {
    it('gives ECharts the markup of the tip', () => {
        const formatter = tipFormatter((p: { name: string }) => tip(strong(p.name), 'ok'));
        expect(formatter({ name: EVIL })).toBe(`<b>${SHOWN}</b><br/>ok`);
    });

    it('shows nothing when there is no tip', () => {
        expect(tipFormatter(() => null)({})).toBe('');
        expect(tipFormatter(() => undefined)({})).toBe('');
    });

    it('escapes what is not a tip, whatever the types were told', () => {
        const raw = tipFormatter(() => EVIL as unknown as Tip);
        expect(raw({})).toBe(SHOWN);
        const lookalike = tipFormatter(() => ({ html: EVIL }) as unknown as Tip);
        expect(lookalike({})).not.toContain('<img');
    });
});

// The guard: every tooltip formatter under `src` is made by
// `tipFormatter`, and nothing else under `src` writes markup into the
// page from a string.

interface Finding {
    file: string;
    line: number;
    what: string;
}

/** The index of the brace that closes the one at `open`. */
function closing(text: string, open: number): number {
    let depth = 0;
    for (let i = open; i < text.length; i += 1) {
        if (text[i] === '{') depth += 1;
        if (text[i] === '}') depth -= 1;
        if (depth === 0) return i;
    }
    return text.length;
}

const lineAt = (text: string, index: number) => text.slice(0, index).split('\n').length;

const blank = (text: string) => text.replace(/[^\n]/g, ' ');

/** `code` with its comments blanked: prose about a tooltip is not one. */
function uncommented(code: string): string {
    return code.replace(/\/\*[\s\S]*?\*\//g, blank).replace(/(^|\s)\/\/.*/g, blank);
}

/** The code of a file: a Svelte component is read for its scripts only. */
function scriptOf(file: string, text: string): string {
    if (!file.endsWith('.svelte')) return uncommented(text);
    return uncommented(scriptsOf(text));
}

function scriptsOf(text: string): string {
    // Everything outside a script is blanked, so line numbers stay true.
    let out = '';
    let at = 0;
    for (const m of text.matchAll(/(<script[^>]*>)([\s\S]*?)<\/script>/g)) {
        out += blank(text.slice(at, m.index + m[1].length)) + m[2];
        at = m.index + m[1].length + m[2].length;
    }
    return out + blank(text.slice(at));
}

const SINKS = /\{@html|\.innerHTML|\.outerHTML|insertAdjacentHTML|document\.write/g;
// A tag, closing or opening, with or without attributes.
const ATTRIBUTE = /\s+[\w:-]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'<>]+))?/;
const MARKUP = new RegExp(
    `<br\\s*\\/?>|<\\/?(?:b|i|em|strong|span|div|img|a|p|script|svg)(?:${ATTRIBUTE.source})*\\s*\\/?>`,
    'g',
);

// A key written as a string, or computed from one, is the same key.
const QUOTED_KEY = /(\[\s*)?(["'`])(tooltip|formatter|valueFormatter)\2(\s*\])?(?=\s*:)/g;

/** `code` with such keys written plainly, every index kept. */
function plainKeys(code: string): string {
    return code.replace(QUOTED_KEY, (_all, pre: string = '', _quote, name: string, post = '') => {
        return `${blank(pre)} ${name} ${blank(String(post))}`;
    });
}

/** The index after the parenthesis closing the one at `open`; strings are skipped. */
function afterCall(text: string, open: number): number {
    let depth = 0;
    let quote = '';
    for (let i = open; i < text.length; i += 1) {
        const c = text[i];
        if (quote !== '') {
            if (c === '\\') i += 1;
            else if (c === quote) quote = '';
            continue;
        }
        if (c === '"' || c === "'" || c === '`') quote = c;
        if (c === '(') depth += 1;
        if (c === ')') depth -= 1;
        if (depth === 0) return i + 1;
    }
    return text.length;
}

/** The members of an object body themselves: what is nested in them is blanked. */
function shallow(body: string): string {
    let depth = 0;
    let out = '';
    for (const c of body) {
        if ('})]'.includes(c)) depth -= 1;
        out += depth <= 1 || c === '\n' ? c : ' ';
        if ('{(['.includes(c)) depth += 1;
    }
    return out;
}

// `x.tooltip` or `x["tooltip"]`, then any members of it, then `=`.
const TOOLTIP_MEMBER = /(?:\.\s*tooltip\b|\[\s*(["'`])tooltip\1\s*\])/.source;
const ASSIGNED = new RegExp(
    `${TOOLTIP_MEMBER}(?:\\s*\\.\\s*[\\w$]+|\\s*\\[[^\\]]*\\])*\\s*=(?![=>])`,
    'g',
);
const OBJECT_ASSIGN = new RegExp(`Object\\s*\\.\\s*assign\\s*\\(\\s*[^,)]*${TOOLTIP_MEMBER}`, 'g');

/** What in `text` could put unescaped text into a tooltip or the page. */
function findings(file: string, text: string): Finding[] {
    const out: Finding[] = [];
    const add = (index: number, what: string) =>
        out.push({ file, line: lineAt(text, index), what });
    for (const m of text.matchAll(SINKS)) add(m.index, `${m[0]} sets markup from a string`);
    const code = plainKeys(scriptOf(file, text));
    for (const m of code.matchAll(/(?<![.\w])tooltip\s*:\s*\{/g)) {
        const open = m.index + m[0].length - 1;
        const body = code.slice(open, closing(code, open) + 1);
        // The formatter is one call of tipFormatter and nothing after it.
        for (const f of body.matchAll(/\b(formatter|valueFormatter)\s*:/g)) {
            const rest = body.slice(f.index + f[0].length);
            const call = /^\s*tipFormatter\(/.exec(rest);
            const end = call ? afterCall(rest, call[0].length - 1) : 0;
            if (call && /^\s*(?:,|\}|$)/.test(rest.slice(end))) continue;
            add(open + f.index, `tooltip ${f[1]} is not made by tipFormatter`);
        }
        // A shorthand or a method.
        for (const f of body.matchAll(/\b(formatter|valueFormatter)\s*[,}(]/g)) {
            add(open + f.index, `tooltip ${f[1]} is not made by tipFormatter`);
        }
        // A spread brings members this check does not see.
        for (const f of shallow(body).matchAll(/\.\.\.\s*([^,}\n]*)/g)) {
            if (/^[A-Za-z_$][\w$]*\.tooltip\s*$/.test(f[1])) continue;
            add(open + f.index, 'tooltip spreads something that is not a theme tooltip');
        }
    }
    // A tooltip changed after it was written is not read above.
    for (const m of code.matchAll(ASSIGNED)) add(m.index, 'tooltip is assigned to here');
    for (const m of code.matchAll(OBJECT_ASSIGN)) {
        add(m.index, 'tooltip is changed by Object.assign here');
    }
    // A tooltip set from a variable hides its formatter from this check.
    for (const m of code.matchAll(/(?<![.\w])tooltip\s*:(?!\s*\{)\s*([^\n,}]*)/g)) {
        if (/^[A-Za-z]+\.tooltip\s*$/.test(m[1])) continue;
        add(m.index, 'tooltip is not written as an object here');
    }
    for (const m of code.matchAll(/(?<![.\w])tooltip\s*[,}]/g)) {
        add(m.index, 'tooltip is not written as an object here');
    }
    for (const m of code.matchAll(MARKUP)) add(m.index, `${m[0]} is markup in a string`);
    return out;
}

const SRC = fileURLToPath(new URL('..', import.meta.url));

function sources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return sources(path);
        const code = /\.(ts|svelte)$/.test(entry.name) && !/\.test\.ts$/.test(entry.name);
        return code ? [path] : [];
    });
}

describe('the tooltip guard', () => {
    const at = (text: string) => findings('x.ts', text).map((f) => `${f.line}: ${f.what}`);

    it('reports a formatter that builds its own string', () => {
        const text =
            'const o = {\n  tooltip: {\n    ...t.tooltip,\n    formatter: (p) => `${p.name}`,\n  },\n};';
        expect(at(text)).toEqual(['4: tooltip formatter is not made by tipFormatter']);
        expect(at('x = { tooltip: { formatter: "{b}" } }')).toHaveLength(1);
        expect(at('x = { tooltip: { formatter } }')).toHaveLength(1);
        expect(at('x = { tooltip: { valueFormatter: (v) => v } }')).toHaveLength(1);
        expect(at('x = { tooltip: { axisPointer: { label: {} }, formatter: f } }')).toHaveLength(1);
    });

    it('reports a tooltip it cannot read', () => {
        expect(at('x = { tooltip: mine }')).toEqual([
            '1: tooltip is not written as an object here',
        ]);
        expect(at('x = { tooltip }')).toHaveLength(1);
        expect(at('x = { tooltip: tips() }')).toHaveLength(1);
    });

    it('reports markup in code and a sink for it', () => {
        expect(at('const s = `<b>${name}</b>`;')).toHaveLength(2);
        expect(at('const s = a + "<br/>" + b;')).toHaveLength(1);
        expect(at('el.innerHTML = s;')).toHaveLength(1);
        expect(findings('x.svelte', '<p>{@html s}</p>')).toHaveLength(1);
    });

    it('reports a tooltip key written as a string or computed', () => {
        expect(at('x = { "tooltip": { formatter } }')).toHaveLength(1);
        expect(at("x = { 'tooltip': { formatter: f } }")).toHaveLength(1);
        expect(at('x = { ["tooltip"]: { formatter: f } }')).toHaveLength(1);
        expect(at('x = { ["tooltip"]: mine }')).toHaveLength(1);
        expect(at('x = { [`tooltip`]: mine }')).toHaveLength(1);
        expect(at('x = { tooltip: { "formatter": f } }')).toHaveLength(1);
        expect(at('x = { tooltip: { ["valueFormatter"]: f } }')).toHaveLength(1);
    });

    it('reports a formatter written as a method', () => {
        expect(at('x = { tooltip: { formatter(p) { return p.name; } } }')).toHaveLength(1);
    });

    it('reports a tooltip changed after it was written', () => {
        expect(at('opt.tooltip.formatter = f;')).toEqual(['1: tooltip is assigned to here']);
        expect(at('opt.tooltip = mine;')).toHaveLength(1);
        expect(at('opt["tooltip"].formatter = f;')).toHaveLength(1);
        expect(at("opt.tooltip['formatter'] = f;")).toHaveLength(1);
        expect(at('opt.series[0].tooltip.formatter = f;')).toHaveLength(1);
        expect(at('if (opt.tooltip === mine || opt.tooltip == null) {}')).toEqual([]);
    });

    it('reports a tooltip filled in by Object.assign', () => {
        expect(at('Object.assign(o.tooltip, { formatter: f });')).toEqual([
            '1: tooltip is changed by Object.assign here',
        ]);
        expect(at('Object.assign(o["tooltip"], more);')).toHaveLength(1);
        expect(at('Object.assign(\n  o.series[0].tooltip,\n  more,\n);')).toHaveLength(1);
    });

    it('reports a tooltip that spreads what it cannot read', () => {
        expect(at('x = { tooltip: { ...mine } }')).toEqual([
            '1: tooltip spreads something that is not a theme tooltip',
        ]);
        expect(at('x = { tooltip: { ...t.tooltip, ...mine() } }')).toHaveLength(1);
        expect(at('x = { tooltip: { trigger: "axis", ...a.b.c } }')).toHaveLength(1);
    });

    it('reports a formatter that is tipFormatter and something more', () => {
        expect(at('x = { tooltip: { formatter: tipFormatter(a) || raw } }')).toEqual([
            '1: tooltip formatter is not made by tipFormatter',
        ]);
        expect(at('x = { tooltip: { formatter: tipFormatter(a) ?? raw } }')).toHaveLength(1);
        expect(at('x = { tooltip: { formatter: tipFormatter(a).bind(raw) } }')).toHaveLength(1);
        expect(at('x = { tooltip: { formatter: tipFormatter(a)\n  || raw,\n} }')).toHaveLength(1);
        expect(at('x = { tooltip: { formatter: c ? tipFormatter(a) : raw } }')).toHaveLength(1);
    });

    it('reports an opening tag with attributes', () => {
        expect(at('const s = `<span style="color:${c}">`;')).toHaveLength(1);
        expect(at("const s = '<img src=x onerror=a()>';")).toHaveLength(1);
        expect(at('const s = `<a href=${url}>`;')).toHaveLength(1);
        expect(at('const s = `<div class="x" hidden>`;')).toHaveLength(1);
        expect(at('const s = "<svg onload=a()/>";')).toHaveLength(1);
        expect(at("const s = '<b\tclass=x>';")).toHaveLength(1);
    });

    it('accepts a formatter made by tipFormatter and a label formatter', () => {
        const ok = [
            'x = { tooltip: { ...t.tooltip, formatter: tipFormatter((p) => tip(p.name)) } }',
            'x = { tooltip: { ...tokens.tooltip, trigger: "axis" } }',
            'x = { tooltip: t.tooltip, series: [{ tooltip: { show: false } }] }',
            'x = { label: { formatter: (p) => p.name } }',
            'x = { tooltip: { formatter: tipFormatter((p) => tip([p.name, " (", p.v, ")"])) } }',
            'x = { tooltip: { formatter: tipFormatter((p) => tip(...rows(p))),\n} }',
            'x = { tooltip: {\n  ...t.tooltip,\n  formatter: tipFormatter(pointTip),\n} }',
            'const a: Record<string, number> = {}; if (a <b || c> d) {}',
        ];
        for (const text of ok) expect(at(text), text).toEqual([]);
    });

    it('does not read a comment as code', () => {
        expect(at('// the tooltip, which shows <b>names</b>\nconst a = 1;')).toEqual([]);
        expect(at('/** A tooltip: {formatter} */\nconst a = 1;')).toEqual([]);
        expect(at('const a = 1; // tooltip: mine\nx = { tooltip: mine }')).toHaveLength(1);
    });

    it('reads the scripts of a component, not its markup', () => {
        const text = '<script lang="ts">\n  const s = "<br/>";\n</script>\n\n<b>bold</b><br />';
        expect(findings('x.svelte', text).map((f) => f.line)).toEqual([2]);
    });

    it('finds nothing under src', () => {
        const files = sources(SRC);
        expect(files.length).toBeGreaterThan(40);
        const found = files
            .filter((file) => !file.endsWith(join('lib', 'tooltip.ts')))
            .flatMap((file) => findings(relative(SRC, file), readFileSync(file, 'utf8')))
            .map((f) => `${f.file}:${f.line}: ${f.what}`);
        expect(found).toEqual([]);
    });
});

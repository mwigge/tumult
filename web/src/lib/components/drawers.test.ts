import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import type { DoraExpectation, Span, StarNode } from '#lib/types.js';
import DoraEvidence from './DoraEvidence.svelte';
import SpanDrawer from './SpanDrawer.svelte';
import StarDrawer from './StarDrawer.svelte';

// Every drawer of Krönika is the Drawer component: a native <dialog> that
// its heading names. No other file draws a drawer of its own.

const SRC = fileURLToPath(new URL('../..', import.meta.url));

const html = (body: string) => body.replace(/<!--[^>]*-->/g, '');

/** The text of the element that names the dialog of `body`, or null. */
function nameOf(body: string): string | null {
    const dialog = body.match(/<dialog[^>]*\saria-labelledby="([^"]+)"/);
    if (!dialog) return null;
    const heading = body.match(new RegExp(`<[a-z0-9]+[^>]*\\sid="${dialog[1]}"[^>]*>([^<]*)<`));
    return heading ? heading[1].trim() : null;
}

const span: Span = {
    ts_ns: 1_700_000_000_000_000_000,
    trace_id: 't-1',
    span_id: 'action',
    parent_span_id: 'root',
    span_name: 'resilience.action',
    span_kind: 'Internal',
    duration_ns: 3_000_000_000,
    status_code: 'Ok',
    status_message: '',
    service_name: 'tumult',
    fault_type: null,
    fault_subtype: null,
    experiment_id: 'run-1',
    span_attrs: {},
    events: [],
};

const row: DoraExpectation = {
    id: 'dora.art24.1',
    article: '24',
    cite: 'Art. 24(1)',
    title: 'General requirements for testing',
    weight: 1,
    expected: 'operating',
    actual: 'designed',
    gap: 2,
    standing: 'gap',
    criticality: 4,
    priority: 1,
    short: [],
    oldest_evidence_ns: null,
    oldest_evidence_date: null,
    days_until_operating: null,
    evidence: { id: 'ev-1', source: 'run' },
    reason: null,
    scope_basis: 'regulation',
    window_days: null,
};

const node: StarNode = {
    id: 'sys:demo/message-queue',
    kind: 'system',
    label: 'message-queue',
    parent: null,
    env: 'demo',
    tier: null,
    org_kind: null,
    criticality_weight: 1,
    conformance: null,
    last_seen_ns: null,
    posture: null,
    evidence: { runs: 0, failed: 0, manual: 0, last_run_ns: null, by_discipline: {} },
    gap: null,
    risks: {
        open: 0,
        suppressed: 0,
        worst: null,
        suppression_expires_ns: null,
        suppressed_by: null,
        suppression_reason: null,
        recommended: [],
    },
    slo: null,
    slo_count: 0,
    dora: null,
    maturity: null,
    conflict: false,
    flags: [],
    rollup: null,
    declared: false,
    children: 0,
    members: null,
};

const close = () => {};

describe('every drawer', () => {
    it('is a dialog that the name of its span names', () => {
        const body = html(render(SpanDrawer, { props: { span, logs: [], onclose: close } }).body);
        expect(nameOf(body)).toBe('resilience.action');
    });

    it('is a dialog that the title of its requirement names', () => {
        const body = html(render(DoraEvidence, { props: { row, lei: '', onclose: close } }).body);
        expect(nameOf(body)).toBe('General requirements for testing');
    });

    it('is a dialog that the label of its node names', () => {
        const props = { node, range: '7d', onclose: close, onfocus: close };
        const body = html(render(StarDrawer, { props }).body);
        expect(nameOf(body)).toBe('message-queue');
    });
});

/** The Svelte and CSS files under src/, with paths from src/. */
function sources(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return sources(path);
        return /\.(svelte|css)$/.test(entry.name) ? [relative(SRC, path)] : [];
    });
}

describe('a drawer of its own', () => {
    it('is in no file but Drawer.svelte: nothing else is fixed to the window or is a dialog', () => {
        const own = sources(SRC).filter((file) => {
            if (file === 'lib/components/Drawer.svelte') return false;
            const text = readFileSync(join(SRC, file), 'utf8');
            return /position:\s*fixed|<dialog\b|role="dialog"/.test(text);
        });
        expect(own, 'use Drawer.svelte for a drawer').toEqual([]);
    });
});

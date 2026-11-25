import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import TabPanel from './TabPanel.svelte';
import Tabs from './Tabs.svelte';

const html = (component: typeof Tabs | typeof TabPanel, props: Record<string, unknown>) =>
    render(component as never, { props: props as never }).body.replace(/<!--[^>]*-->/g, '');

const SECTIONS = [
    { value: 'entry', label: 'New entry' },
    { value: 'plans', label: 'Plans' },
    { value: 'import', label: 'Import' },
];

const tabs = (props: Record<string, unknown> = {}) =>
    html(Tabs, {
        id: 'manual',
        label: 'manual sections',
        options: SECTIONS,
        value: 'plans',
        onchange: () => {},
        ...props,
    });

/** The opening tag of the tab of `label`. */
const tabOf = (out: string, label: string) =>
    out.match(new RegExp(`<button[^>]*>${label}</button>`))?.[0] ?? '';

describe('the tabs of a page', () => {
    it('are a tab list with the name it is given', () => {
        expect(tabs()).toMatch(/^<div class="seg" role="tablist" aria-label="manual sections">/);
    });

    it('are tabs that each name the panel they control', () => {
        const out = tabs();
        for (const { value, label } of SECTIONS) {
            const tab = tabOf(out, label);
            expect(tab).toContain('role="tab"');
            expect(tab).toContain(`id="manual-tab-${value}"`);
            expect(tab).toContain('aria-controls="manual-panel"');
        }
    });

    it('select the tab of the value and keep only that tab in the tab order', () => {
        const out = tabs();
        expect(tabOf(out, 'Plans')).toContain('aria-selected="true"');
        expect(tabOf(out, 'Plans')).toContain('tabindex="0"');
        expect(tabOf(out, 'Plans')).toContain('class="active"');
        for (const label of ['New entry', 'Import']) {
            expect(tabOf(out, label)).toContain('aria-selected="false"');
            expect(tabOf(out, label)).toContain('tabindex="-1"');
            expect(tabOf(out, label)).not.toContain('class=');
        }
    });

    it('keep the first tab in the tab order when no tab has the value', () => {
        const out = tabs({ value: 'gone' });
        expect(tabOf(out, 'New entry')).toContain('tabindex="0"');
        expect(out).not.toContain('aria-selected="true"');
    });

    it('keep the class of a page and the class that marks the chosen tab', () => {
        const out = tabs({ class: 'views', selectedClass: 'on', value: 'entry' });
        expect(out).toMatch(/^<div class="views" role="tablist"/);
        expect(tabOf(out, 'New entry')).toContain('class="on"');
    });
});

describe('the panel of the tabs', () => {
    it('is a tab panel named by the selected tab, that a key can reach', () => {
        const out = html(TabPanel, { id: 'manual', value: 'plans' });
        expect(out).toBe(
            '<div role="tabpanel" id="manual-panel" aria-labelledby="manual-tab-plans" tabindex="0"></div>',
        );
    });
});

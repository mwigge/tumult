import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';

import EChart from './EChart.svelte';

// The server render is the host element as written, before any chart
// is drawn into it: what assistive technology is told about the chart.
function host(props: { label?: string; id?: string }): string {
    return render(EChart, { props: { option: {}, ...props } }).body;
}

describe('the chart host element', () => {
    it('is an image with a name when it has a label', () => {
        const html = host({ label: 'Runs per fault type', id: 'faults' });
        expect(html).toContain('role="img"');
        expect(html).toContain('aria-label="Runs per fault type"');
        expect(html).toContain('data-chart="faults"');
    });

    it('has no role and no name without a label', () => {
        for (const html of [host({}), host({ label: '' })]) {
            expect(html).toContain('<div');
            expect(html).not.toContain('role=');
            expect(html).not.toContain('aria-label');
        }
    });
});

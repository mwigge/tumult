import { describe, expect, it } from 'vitest';

import { panelId, tabAfterKey, tabId, withParam } from './tabs';

describe('a key on a tab', () => {
    it('moves right and left with the arrow keys, and wraps at the ends', () => {
        expect(tabAfterKey('ArrowRight', 0, 3)).toBe(1);
        expect(tabAfterKey('ArrowRight', 2, 3)).toBe(0);
        expect(tabAfterKey('ArrowLeft', 1, 3)).toBe(0);
        expect(tabAfterKey('ArrowLeft', 0, 3)).toBe(2);
    });

    it('goes to the first tab with Home and to the last with End', () => {
        expect(tabAfterKey('Home', 2, 3)).toBe(0);
        expect(tabAfterKey('End', 0, 3)).toBe(2);
    });

    it('does nothing for another key', () => {
        for (const key of ['Enter', ' ', 'Tab', 'ArrowDown', 'ArrowUp', 'a'])
            expect(tabAfterKey(key, 1, 3)).toBeNull();
    });
});

describe('the ids of tabs and their panel', () => {
    it('are named by the tabs and the value', () => {
        expect(tabId('manual', 'plans')).toBe('manual-tab-plans');
        expect(panelId('manual')).toBe('manual-panel');
    });
});

describe('the URL of a tab', () => {
    it('sets the tab and keeps the other parameters', () => {
        const search = new URLSearchParams('tab=plans&target=payments-db');
        expect(withParam(search, 'tab', 'records')).toBe('?tab=records&target=payments-db');
        expect(search.toString()).toBe('tab=plans&target=payments-db');
    });

    it('adds the tab when the URL has none', () => {
        expect(withParam(new URLSearchParams('lei=X1'), 'view', 'dora')).toBe('?lei=X1&view=dora');
    });

    it('leaves the tab out for no value', () => {
        expect(withParam(new URLSearchParams('view=dora&lei=X1'), 'view', null)).toBe('?lei=X1');
    });
});

import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

import { describe, expect, it } from 'vitest';

// static/early-theme.js runs in the head of app.html, before the app.
// Here it runs as a browser runs it, on a stand-in for the storage and
// for <html>.

const SCRIPT = readFileSync(new URL('../../static/early-theme.js', import.meta.url), 'utf8');

type Storage = { getItem(key: string): string | null };

/** Run the script on `storage`, and give back the data-theme it set on <html>. */
function themeSetFrom(storage: Storage): string | undefined {
    const dataset: { theme?: string } = {};
    runInNewContext(SCRIPT, { localStorage: storage, document: { documentElement: { dataset } } });
    return dataset.theme;
}

const saved = (values: Record<string, string>): Storage => ({
    getItem: (key) => values[key] ?? null,
});

describe('early-theme.js', () => {
    it.each(['dark', 'light', 'auto'])('puts the saved theme %s on <html>', (theme) => {
        expect(themeSetFrom(saved({ 'tumult.theme': theme }))).toBe(theme);
    });

    it('leaves <html> as it is when no theme is saved', () => {
        expect(themeSetFrom(saved({}))).toBeUndefined();
    });

    it('reads only the key the theme button writes', () => {
        expect(themeSetFrom(saved({ theme: 'light' }))).toBeUndefined();
    });

    it('leaves <html> as it is when the saved value is not a theme', () => {
        expect(themeSetFrom(saved({ 'tumult.theme': 'blue' }))).toBeUndefined();
    });

    it('does nothing when the storage cannot be read, as in a private window', () => {
        const blocked: Storage = {
            getItem: () => {
                throw new Error('SecurityError: storage is blocked');
            },
        };
        expect(themeSetFrom(blocked)).toBeUndefined();
    });
});

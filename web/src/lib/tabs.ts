// The keys, ids and URL of tabs (components/Tabs.svelte), after the WAI-ARIA
// tabs pattern.

/**
 * The index of the tab that `key` moves to from the tab at `index` of
 * `count` tabs, or null for a key that moves nowhere. The arrow keys wrap
 * at the ends; Home and End go to the first and the last tab.
 */
export function tabAfterKey(key: string, index: number, count: number): number | null {
    if (key === 'ArrowRight') return (index + 1) % count;
    if (key === 'ArrowLeft') return (index - 1 + count) % count;
    if (key === 'Home') return 0;
    if (key === 'End') return count - 1;
    return null;
}

/** The id of the tab of `value` in the tabs `tabs`. */
export const tabId = (tabs: string, value: string) => `${tabs}-tab-${value}`;

/** The id of the panel of the tabs `tabs`. */
export const panelId = (tabs: string) => `${tabs}-panel`;

/**
 * The query of `search` with `key` set to `value`, or without `key` for
 * null. The other parameters stay as they are.
 */
export function withParam(
    search: { toString(): string },
    key: string,
    value: string | null,
): string {
    const params = new URLSearchParams(search.toString());
    if (value === null) params.delete(key);
    else params.set(key, value);
    return `?${params}`;
}

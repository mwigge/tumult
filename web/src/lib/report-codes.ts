// The short code a compliance report is known by, from its template.

/** The code of each template the daemon makes reports of. */
const CODES: ReadonlyMap<string, string> = new Map([
    ['executive-digest', 'R1'],
    ['evidence-pack', 'R2'],
    ['game-day', 'R3'],
    ['dr-test', 'R4'],
    ['dr-exercise', 'R5'],
]);

/**
 * The code of the report of `type`, the template the daemon names in its
 * meta. A template the page does not know shows its own name, not the code
 * of another report.
 */
export function templateCode(type: string): string {
    return CODES.get(type) ?? type;
}

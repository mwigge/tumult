// Row labels of the coverage matrix. A target tested in two environments
// has two rows; the environment is added to tell them apart.

export interface CoverageRow {
    target: string;
    environment: string | null;
}

/** One label per row, in order: the target, plus its environment when the name repeats. */
export function coverageLabels(rows: CoverageRow[]): string[] {
    const seen = new Map<string, number>();
    for (const row of rows) seen.set(row.target, (seen.get(row.target) ?? 0) + 1);
    return rows.map((row) => {
        if ((seen.get(row.target) ?? 0) < 2) return row.target;
        return `${row.target} (${row.environment ?? 'no environment'})`;
    });
}

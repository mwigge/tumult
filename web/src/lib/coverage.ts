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

const DISCIPLINE_LABELS: Record<string, string> = {
    performance: 'load / performance',
    robustness: 'robustness / dr',
};

/**
 * The name a discipline is shown under. Load and performance are one
 * discipline, and disaster recovery is part of robustness.
 */
export function disciplineLabel(kind: string): string {
    return DISCIPLINE_LABELS[kind] ?? kind;
}

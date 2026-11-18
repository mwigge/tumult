// The two scores from 0 to 100, each with one name and one line that says
// what it measures. Every page that shows a score takes its words from here.

/** The score of `/api/scores` and the gate: the latest runs of the experiments. */
export const COMPLIANCE_SCORE = 'Compliance score';

/** What the compliance score measures; the digest says it in the same words. */
export const COMPLIANCE_SCORE_LINE =
    'Latest run of each experiment: pass 100, pass older than 30 days 75, failure 50, never run 0. Averaged per target.';

/** The continuous score of `/api/posture`, on the Resilience page. */
export const RESILIENCE_SCORE = 'Resilience score';

/** What the resilience score measures. */
export const RESILIENCE_SCORE_LINE =
    'Continuous score: objective margins, discipline coverage, evidence freshness and recovery against target, per target and criticality weighted.';

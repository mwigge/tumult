// The manual registry on the page: the form of a DR exercise and of a plan,
// what they send to the API and how a stored record reads back into them.
// Pure functions, so the page and its tests share them. The API checks every
// value; the page sends what was typed and shows the reason of a refusal.

import type {
    ManualAction,
    ManualDetail,
    ManualImportInput,
    ManualLink,
    ManualPlan,
    ManualPlanInput,
    ManualRecordInput,
    ManualStep,
    ManualTester,
    ManualTlpt,
} from './types';

/** The exercise types of the store, in its order. */
export const EXERCISE_TYPES = [
    'gameday',
    'tabletop',
    'failover',
    'pentest',
    'tlpt',
    'drill',
    'restore',
    'backup',
    'other',
] as const;

export const OUTCOMES = ['passed', 'partial', 'failed', 'inconclusive'] as const;

export const STEP_RESULTS = ['done', 'failed', 'skipped'] as const;

/** How much a finding or an action matters (DORA Art. 24(5)). */
export const SEVERITIES = ['low', 'medium', 'high', 'critical'] as const;

/** Who ran a test: from inside the entity or from outside it (DORA Art. 24(4)). */
export const TESTER_KINDS = ['internal', 'external'] as const;

/** What retested an action: an automated run or another record. */
export const RETEST_KINDS = ['run', 'record'] as const;

/** A number input: a number, or a blank as the empty text or nothing. */
export type NumberField = number | string | null | undefined;

export interface ParticipantRow {
    name: string;
    role: string;
}

export interface StepRow {
    title: string;
    actor: string;
    /** datetime-local values, to the second. */
    started: string;
    ended: string;
    result: string;
    note: string;
}

export interface ActionRow {
    title: string;
    owner: string;
    /** A date: the action is due by the end of that day. */
    due: string;
    ticket: string;
    severity: string;
    /** The run or the record that retested it: a kind and an id. */
    retest_kind: string;
    retest_id: string;
}

export const emptyParticipant = (): ParticipantRow => ({ name: '', role: '' });

export const emptyStep = (): StepRow => ({
    title: '',
    actor: '',
    started: '',
    ended: '',
    result: '',
    note: '',
});

export const emptyAction = (): ActionRow => ({
    title: '',
    owner: '',
    due: '',
    ticket: '',
    severity: '',
    retest_kind: '',
    retest_id: '',
});

export const emptyForm = () => ({
    experiment_name: '',
    exercise_type: 'gameday',
    executed: '',
    hypothesis: '',
    method: '',
    outcome_status: 'passed',
    hypothesis_met: 'unknown',
    findings: '',
    target_system: '',
    target_environment: '',
    blast_radius: '',
    recovery_time_s: '' as NumberField,
    duration_s: '' as NumberField,
    framework_refs: '',
    renewal: '',
    attestation: '',
    plan_id: '',
    rto_target_s: '' as NumberField,
    rpo_target_s: '' as NumberField,
    rpo_measured_s: '' as NumberField,
    /** `unknown` not checked, `yes` complete, `no` loss or corruption found. */
    data_verified: 'unknown',
    participants: [] as ParticipantRow[],
    steps: [] as StepRow[],
    actions: [] as ActionRow[],
    /** The links of a draft, sent back as they are. */
    links: [] as ManualLink[],
    findings_severity: '',
    tester_name: '',
    tester_kind: '',
    /** The TLPT block, sent for a record of type `tlpt` only. */
    tlpt_attestation_ref: '',
    /** A date `YYYY-MM-DD`: the attestation was issued that day. */
    tlpt_attested: '',
    tlpt_summary_ref: '',
});

export type ManualForm = ReturnType<typeof emptyForm>;

// --- times ------------------------------------------------------------------

/** A datetime-local value as unix nanoseconds; a blank is null. */
export function toNs(local: string): number | null {
    if (!local) return null;
    const ms = new Date(local).getTime();
    return Number.isNaN(ms) ? null : ms * 1_000_000;
}

/** The start of a local day `YYYY-MM-DD`. */
export function dayStartNs(date: string): number | null {
    return date ? toNs(`${date}T00:00`) : null;
}

/** The end of a local day `YYYY-MM-DD`, so a due date holds all of that day. */
export function dayEndNs(date: string): number | null {
    return date ? toNs(`${date}T23:59:59.999`) : null;
}

/** A time as a datetime-local value, to the minute or to the second. */
export function localInput(ns: number | null | undefined, seconds = false): string {
    if (!ns) return '';
    const date = new Date(ns / 1_000_000);
    const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
    return shifted.toISOString().slice(0, seconds ? 19 : 16);
}

/** The local day of a time, `YYYY-MM-DD`. */
export function localDay(ns: number | null | undefined): string {
    return localInput(ns).slice(0, 10);
}

// --- the record form ------------------------------------------------------------

const text = (value: string) => value.trim() || null;

/** A number field as seconds: a blank is null, a zero stays a zero. */
export function seconds(value: NumberField): number | null {
    if (value === null || value === undefined || value === '') return null;
    const n = Number(value);
    return Number.isNaN(n) ? null : n;
}

const blank = (row: object) => Object.values(row).every((v) => String(v ?? '').trim() === '');

const DATA_VERIFIED: Record<string, boolean | null> = { yes: true, no: false, unknown: null };

/** What the page checks before it sends a record: the fields it cannot send
    without, and the environment scopes the API would refuse. */
export function checkForm(form: ManualForm, actor: string, environments: string[]): string | null {
    if (!toNs(form.executed)) return 'executed date/time is required';
    if (!form.experiment_name.trim()) return 'experiment name is required';
    if (!form.hypothesis.trim()) return 'hypothesis is required';
    if (environments.length && !environments.includes(form.target_environment.trim())) {
        return `Choose a target environment allowed by your account: ${environments.join(', ')}`;
    }
    if (!form.method.trim()) return 'method is required';
    if (!form.attestation.trim()) return 'attestation is required';
    if (!actor.trim()) return 'set the "acting as" name first';
    return null;
}

function stepOf(row: StepRow): ManualStep {
    return {
        title: row.title.trim(),
        actor: text(row.actor),
        started_at_ns: toNs(row.started),
        ended_at_ns: toNs(row.ended),
        result: row.result || null,
        note: text(row.note),
    };
}

function actionOf(row: ActionRow): ManualAction {
    const retest = row.retest_id.trim();
    return {
        title: row.title.trim(),
        owner: row.owner.trim(),
        due_at_ns: dayEndNs(row.due),
        ticket: text(row.ticket),
        severity: row.severity || null,
        retest: retest ? { kind: row.retest_kind, id: retest } : null,
    };
}

/** The tester the form names; the API checks its kind. */
function testerOf(form: ManualForm): ManualTester | null {
    const name = form.tester_name.trim();
    return name || form.tester_kind ? { name, kind: form.tester_kind } : null;
}

/** The TLPT block of a record of type `tlpt`, when the form holds any of it. */
function tlptOf(form: ManualForm): ManualTlpt | null {
    if (form.exercise_type !== 'tlpt') return null;
    const tlpt = {
        attestation_ref: text(form.tlpt_attestation_ref),
        attested_at_ns: dayStartNs(form.tlpt_attested),
        summary_report_ref: text(form.tlpt_summary_ref),
    };
    return Object.values(tlpt).some((v) => v !== null) ? tlpt : null;
}

/** The body of a create or an edit. It sends `actions`, never the older
    `action_items` list. */
export function formToRecord(form: ManualForm, actor: string): ManualRecordInput {
    return {
        experiment_name: form.experiment_name.trim(),
        exercise_type: form.exercise_type,
        executed_at_ns: toNs(form.executed) ?? 0,
        hypothesis: form.hypothesis.trim(),
        method: form.method.trim(),
        outcome_status: form.outcome_status,
        hypothesis_met: form.hypothesis_met === 'unknown' ? null : form.hypothesis_met === 'met',
        findings: text(form.findings),
        target_system: text(form.target_system),
        target_environment: text(form.target_environment),
        blast_radius: text(form.blast_radius),
        recovery_time_s: seconds(form.recovery_time_s),
        duration_s: seconds(form.duration_s),
        entered_by: actor.trim(),
        attestation: form.attestation.trim(),
        renewal_due_ns: dayStartNs(form.renewal),
        framework_refs: form.framework_refs
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean),
        rto_target_s: seconds(form.rto_target_s),
        rpo_target_s: seconds(form.rpo_target_s),
        rpo_measured_s: seconds(form.rpo_measured_s),
        data_verified: DATA_VERIFIED[form.data_verified] ?? null,
        participants: form.participants
            .filter((p) => !blank(p))
            .map((p) => ({ name: p.name.trim(), role: text(p.role) })),
        steps: form.steps.filter((s) => !blank(s)).map(stepOf),
        actions: form.actions.filter((a) => !blank(a)).map(actionOf),
        links: form.links,
        plan_id: form.plan_id || null,
        findings_severity: form.findings_severity || null,
        tester: testerOf(form),
        tlpt: tlptOf(form),
    };
}

const numberText = (value: number | null | undefined) => (value == null ? '' : String(value));

/** A stored record as the form. A plain action of the older list becomes an
    action that still needs an owner. */
export function detailToForm(detail: ManualDetail): ManualForm {
    const rec = detail.experiment;
    const older = Array.isArray(rec.action_items)
        ? rec.action_items.filter((item): item is string => typeof item === 'string')
        : [];
    const verified = rec.data_verified ?? null;
    return {
        ...emptyForm(),
        experiment_name: rec.experiment_name,
        exercise_type: rec.exercise_type,
        executed: localInput(rec.executed_at_ns),
        hypothesis: rec.hypothesis,
        method: rec.method,
        outcome_status: rec.outcome_status,
        hypothesis_met:
            rec.hypothesis_met === null ? 'unknown' : rec.hypothesis_met ? 'met' : 'not-met',
        findings: rec.findings ?? '',
        target_system: rec.target_system ?? '',
        target_environment: rec.target_environment ?? '',
        blast_radius: rec.blast_radius ?? '',
        recovery_time_s: numberText(rec.recovery_time_s),
        duration_s: numberText(rec.duration_s),
        framework_refs: (rec.framework_refs ?? []).join(', '),
        renewal: localDay(rec.renewal_due_ns),
        attestation: rec.attestation,
        plan_id: rec.plan_id ?? '',
        rto_target_s: numberText(rec.rto_target_s),
        rpo_target_s: numberText(rec.rpo_target_s),
        rpo_measured_s: numberText(rec.rpo_measured_s),
        data_verified: verified === null ? 'unknown' : verified ? 'yes' : 'no',
        participants: (rec.participants ?? []).map((p) => ({ name: p.name, role: p.role ?? '' })),
        steps: (detail.steps ?? []).map((s) => ({
            title: s.title,
            actor: s.actor ?? '',
            started: localInput(s.started_at_ns, true),
            ended: localInput(s.ended_at_ns, true),
            result: s.result ?? '',
            note: s.note ?? '',
        })),
        actions: [
            ...(detail.actions ?? []).map((a) => ({
                title: a.title,
                owner: a.owner,
                due: localDay(a.due_at_ns),
                ticket: a.ticket ?? '',
                severity: a.severity ?? '',
                retest_kind: a.retest_kind ?? '',
                retest_id: a.retest_id ?? '',
            })),
            ...older.map((title) => ({ ...emptyAction(), title })),
        ],
        links: (detail.links ?? []).map((l) => ({
            kind: l.kind,
            linked_id: l.linked_id,
            label: l.label ?? null,
        })),
        findings_severity: rec.findings_severity ?? '',
        tester_name: rec.tester ?? '',
        tester_kind: rec.tester_kind ?? '',
        tlpt_attestation_ref: rec.tlpt_attestation_ref ?? '',
        tlpt_attested: localDay(rec.tlpt_attested_at_ns),
        tlpt_summary_ref: rec.tlpt_summary_ref ?? '',
    };
}

// --- plans ------------------------------------------------------------------------

type Target = Pick<ManualForm, 'exercise_type' | 'target_system' | 'target_environment'>;

/** The planned plans a record of `form` can fulfil: the same exercise type,
    and the same target where both the plan and the form name one. */
export function matchingPlans(plans: ManualPlan[], form: Target): ManualPlan[] {
    const fits = (planned: string | null, typed: string) =>
        !planned || !typed.trim() || planned === typed.trim();
    return plans.filter(
        (p) =>
            p.status === 'planned' &&
            p.exercise_type === form.exercise_type &&
            fits(p.target_system, form.target_system) &&
            fits(p.target_environment, form.target_environment),
    );
}

const orBlank = (typed: NumberField | string, planned: number | string | null) =>
    typed === '' || typed == null ? (planned ?? '') : typed;

/** The form with `plan` picked: what the form leaves blank comes from the
    plan, and a plan of a function adds the function link the API asks for. */
export function applyPlan(form: ManualForm, plan: ManualPlan): ManualForm {
    const fn = plan.function_id;
    const linked = form.links.some((l) => l.kind === 'function' && l.linked_id === fn);
    return {
        ...form,
        plan_id: plan.id,
        target_system: String(orBlank(form.target_system, plan.target_system)),
        target_environment: String(orBlank(form.target_environment, plan.target_environment)),
        rto_target_s: orBlank(form.rto_target_s, plan.rto_target_s),
        rpo_target_s: orBlank(form.rpo_target_s, plan.rpo_target_s),
        links:
            fn && !linked
                ? [...form.links, { kind: 'function', linked_id: fn, label: null }]
                : form.links,
    };
}

/** A plan that is still planned after its due date. */
export function isOverdue(plan: ManualPlan, nowNs: number): boolean {
    return plan.status === 'planned' && plan.due_at_ns < nowNs;
}

export const emptyPlanForm = () => ({
    name: '',
    exercise_type: 'failover',
    target_system: '',
    target_environment: '',
    function_id: '',
    scope: '',
    owner: '',
    /** Dates `YYYY-MM-DD`: planned from the start of the day, due by its end. */
    planned: '',
    due: '',
    interval_days: '' as NumberField,
    rto_target_s: '' as NumberField,
    rpo_target_s: '' as NumberField,
});

export type PlanForm = ReturnType<typeof emptyPlanForm>;

export function planToForm(plan: ManualPlan): PlanForm {
    return {
        name: plan.name,
        exercise_type: plan.exercise_type,
        target_system: plan.target_system ?? '',
        target_environment: plan.target_environment ?? '',
        function_id: plan.function_id ?? '',
        scope: plan.scope ?? '',
        owner: plan.owner,
        planned: localDay(plan.planned_at_ns),
        due: localDay(plan.due_at_ns),
        interval_days: numberText(plan.interval_days),
        rto_target_s: numberText(plan.rto_target_s),
        rpo_target_s: numberText(plan.rpo_target_s),
    };
}

function planFields(form: PlanForm): ManualPlanInput {
    const due = dayEndNs(form.due);
    return {
        name: form.name.trim(),
        exercise_type: form.exercise_type,
        target_system: text(form.target_system),
        target_environment: text(form.target_environment),
        function_id: text(form.function_id),
        scope: text(form.scope),
        owner: form.owner.trim(),
        planned_at_ns: dayStartNs(form.planned),
        ...(due === null ? {} : { due_at_ns: due }),
        interval_days: seconds(form.interval_days),
        rto_target_s: seconds(form.rto_target_s),
        rpo_target_s: seconds(form.rpo_target_s),
    };
}

/** The body of `POST /api/manual/plans`. A blank due date is left out, so
    the API names it. A blank interval lets the API pick the one of the type. */
export function planRequest(form: PlanForm, actor: string): ManualPlanInput {
    return { ...planFields(form), created_by: actor.trim() };
}

/** The body of `PATCH /api/manual/plans/{id}`: every field, a blank sent as
    `null` so the API clears it. */
export function planPatch(form: PlanForm, actor: string): ManualPlanInput {
    return { ...planFields(form), updated_by: actor.trim() };
}

// --- import -----------------------------------------------------------------------

/** The body of `POST /api/manual/import` from the text of a JSON file: the
    body the API takes, or a list of records. A record that names no
    `entered_by` gets `actor`, as the form does. */
export function importBody(
    raw: string,
    label: string,
    actor: string,
): { body: ManualImportInput } | { error: string } {
    let parsed: unknown;
    try {
        parsed = JSON.parse(raw);
    } catch (e) {
        return { error: `The import is not JSON: ${errorText(e)}` };
    }
    const body = (Array.isArray(parsed) ? { records: parsed } : parsed) as ManualImportInput;
    if (label.trim() && body && typeof body === 'object' && !('label' in body)) {
        body.label = label.trim();
    }
    if (actor.trim() && Array.isArray(body?.records)) {
        body.records = body.records.map((r) =>
            r && typeof r === 'object' && !('entered_by' in r)
                ? { ...r, entered_by: actor.trim() }
                : r,
        );
    }
    return { body };
}

// --- words ------------------------------------------------------------------------

export function errorText(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
}

/** A file size in bytes, KB or MB. */
export function fmtBytes(n: number | null | undefined): string {
    if (n === null || n === undefined) return '—';
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Where a record stands in its sign-offs. */
export function signoffSummary(
    status: string,
    approvals: number,
    required: number,
    needed: number,
): string {
    if (status === 'verified') return `Verified with ${approvals} of ${required} approvals.`;
    if (status === 'rejected') return 'Rejected.';
    const more = needed === 1 ? '1 more sign-off needed' : `${needed} more sign-offs needed`;
    return `${approvals} of ${required} approvals. ${more}.`;
}

/** A recovery against its target: `met` at or under it, else `missed`. */
export function recoveryVerdict(
    target: number | null | undefined,
    actual: number | null | undefined,
): 'met' | 'missed' | null {
    if (target == null || actual == null) return null;
    return actual <= target ? 'met' : 'missed';
}

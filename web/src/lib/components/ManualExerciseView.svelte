<script lang="ts">
    // The DR detail of a stored record: the recovery against its targets,
    // the participants and the tester, the TLPT block, the steps in order,
    // the actions with owners, severities and retests, and the links to what
    // was tested.
    import { fmtDuration, fmtTs } from '#lib/api.js';
    import { localDay, recoveryVerdict } from '#lib/manual.js';
    import type { ManualDetail } from '#lib/types.js';

    let { detail, planName = '' }: { detail: ManualDetail; planName?: string } = $props();

    const rec = $derived(detail.experiment);
    const steps = $derived(detail.steps ?? []);
    const actions = $derived(detail.actions ?? []);
    const links = $derived(detail.links ?? []);
    const people = $derived(rec.participants ?? []);
    const older = $derived(
        Array.isArray(rec.action_items)
            ? rec.action_items.filter((s): s is string => typeof s === 'string')
            : [],
    );
    const recovery = $derived([
        { name: 'RTO', target: rec.rto_target_s ?? null, actual: rec.recovery_time_s ?? null },
        { name: 'RPO', target: rec.rpo_target_s ?? null, actual: rec.rpo_measured_s ?? null },
    ]);

    const secs = (s: number | null) => (s === null ? '—' : `${s} s`);
    const resultClass = (r: string | null | undefined) =>
        r === 'done' ? 'ok' : r === 'failed' ? 'fail' : 'neutral';
    const took = (start?: number | null, end?: number | null) =>
        start && end ? fmtDuration(end - start) : '';
    const dataText = (v: boolean | null | undefined) =>
        v === true ? 'complete' : v === false ? 'loss or corruption found' : 'not checked';
    const severityClass = (s: string | null | undefined) =>
        s === 'critical' || s === 'high' ? 'fail' : s === 'medium' ? 'warn' : 'neutral';
    const tlpt = $derived(
        rec.tlpt_attestation_ref || rec.tlpt_attested_at_ns || rec.tlpt_summary_ref
            ? {
                  attestation: rec.tlpt_attestation_ref ?? '—',
                  attested: rec.tlpt_attested_at_ns ? localDay(rec.tlpt_attested_at_ns) : '—',
                  summary: rec.tlpt_summary_ref ?? '—',
              }
            : null,
    );
</script>

<div class="exercise">
    <div>
        <h3>Recovery</h3>
        <table class="data compact">
            <thead><tr><th></th><th>Planned</th><th>Actual</th><th></th></tr></thead>
            <tbody>
                {#each recovery as r (r.name)}
                    {@const verdict = recoveryVerdict(r.target, r.actual)}
                    <tr>
                        <td>{r.name}</td>
                        <td class="mono">{secs(r.target)}</td>
                        <td class="mono">{secs(r.actual)}</td>
                        <td>
                            {#if verdict}<span class="badge {verdict === 'met' ? 'ok' : 'fail'}"
                                    >{verdict}</span
                                >{/if}
                        </td>
                    </tr>
                {/each}
            </tbody>
        </table>
        <p class="line">Data verified: <b>{dataText(rec.data_verified)}</b></p>
        {#if rec.findings_severity}
            <p class="line">
                Findings severity:
                <span class="badge {severityClass(rec.findings_severity)}"
                    >{rec.findings_severity}</span
                >
            </p>
        {/if}
        {#if rec.tester}
            <p class="line">
                Tester: <b>{rec.tester}</b>
                {#if rec.tester_kind}<span class="dim">· {rec.tester_kind}</span>{/if}
            </p>
        {/if}
        {#if tlpt}
            <h3>TLPT</h3>
            <p class="line">
                Attestation <b>{tlpt.attestation}</b> of {tlpt.attested}
            </p>
            <p class="line">Test summary report <b>{tlpt.summary}</b></p>
        {/if}
        {#if rec.plan_id}
            <p class="line">Fulfils plan <b>{planName || rec.plan_id}</b></p>
        {/if}
        <h3>Participants</h3>
        {#if people.length === 0}
            <div class="dim">No participants recorded.</div>
        {:else}
            <ul class="plain">
                {#each people as p, i (i)}
                    <li>
                        {p.name}
                        {#if p.role}<span class="dim">· {p.role}</span>{/if}
                    </li>
                {/each}
            </ul>
        {/if}
        {#if links.length > 0}
            <h3>Tested</h3>
            <ul class="plain">
                {#each links as l, i (i)}
                    <li>
                        <span class="badge neutral">{l.kind}</span>
                        <span class="mono">{l.linked_id}</span>
                        {#if l.label}<span class="dim">· {l.label}</span>{/if}
                    </li>
                {/each}
            </ul>
        {/if}
    </div>
    <div>
        <h3>Steps</h3>
        {#if steps.length === 0}
            <div class="dim">No steps recorded.</div>
        {:else}
            <ol class="steps">
                {#each steps as s, i (i)}
                    <li>
                        <span class="badge {resultClass(s.result)}">{s.result ?? 'no result'}</span>
                        {s.title}
                        {#if s.actor}<span class="dim"> · {s.actor}</span>{/if}
                        <div class="dim mono">
                            {s.started_at_ns ? fmtTs(s.started_at_ns) : ''}
                            {s.ended_at_ns ? `→ ${fmtTs(s.ended_at_ns)}` : ''}
                            {took(s.started_at_ns, s.ended_at_ns)
                                ? `· ${took(s.started_at_ns, s.ended_at_ns)}`
                                : ''}
                        </div>
                        {#if s.note}<div class="dim">{s.note}</div>{/if}
                    </li>
                {/each}
            </ol>
        {/if}
        <h3>{rec.exercise_type === 'tlpt' ? 'Remediation plan' : 'Actions'}</h3>
        {#if actions.length === 0 && older.length === 0}
            <div class="dim">No actions recorded.</div>
        {:else}
            <ul class="plain">
                {#each actions as a, i (i)}
                    <li>
                        {a.title} <span class="dim">· {a.owner}</span>
                        {#if a.due_at_ns}<span class="dim">
                                · due {localDay(a.due_at_ns)}</span
                            >{/if}
                        {#if a.ticket}<span class="dim"> · {a.ticket}</span>{/if}
                        {#if a.severity}<span class="badge {severityClass(a.severity)}"
                                >{a.severity}</span
                            >{/if}
                        {#if a.status}<span class="badge neutral">{a.status}</span>{/if}
                        {#if a.retest_id}<div class="dim">
                                retested by {a.retest_kind}
                                <span class="mono">{a.retest_id}</span>
                            </div>{/if}
                    </li>
                {/each}
                {#each older as text, i (i)}
                    <li>{text} <span class="dim">· from the older action list, no owner</span></li>
                {/each}
            </ul>
        {/if}
    </div>
</div>

<style>
    .exercise {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 16px;
        margin-bottom: 12px;
    }
    @media (max-width: 900px) {
        .exercise {
            grid-template-columns: 1fr;
        }
    }
    h3 {
        font-size: 12px;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        color: var(--text-dim);
        margin: 10px 0 6px;
    }
    .compact td,
    .compact th {
        padding: 4px 8px;
    }
    .line {
        margin: 6px 0;
        font-size: 13px;
    }
    .plain,
    .steps {
        margin: 0;
        padding-left: 18px;
    }
    .plain li,
    .steps li {
        margin-bottom: 4px;
    }
    .dim {
        color: var(--text-faint);
        font-size: 12px;
    }
</style>

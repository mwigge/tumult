<script lang="ts">
    // Resilience: the executive opening view. Ranked risks and discipline
    // coverage gaps first (the passive scan runs with no faults), then the
    // continuous resilience score per target (/api/posture). Operators can
    // accept a finding for a while; suppressions are hash-chained server side.
    import { onMount } from 'svelte';
    import { api, fmtAgo } from '#lib/api.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { chartTheme, chartTokens, followTheme } from '#lib/echarts.js';
    import type { LegendItem } from '#lib/legend.js';
    import { coverageLabels, disciplineLabel, stackedLabel } from '#lib/coverage.js';
    import { sparkTrend } from '#lib/org-score.js';
    import {
        COMPLIANCE_SCORE,
        COMPLIANCE_SCORE_LINE,
        RESILIENCE_SCORE,
        RESILIENCE_SCORE_LINE,
    } from '#lib/score-names.js';
    import { strong, tip, tipFormatter } from '#lib/tooltip.js';
    import type {
        ConformanceReport,
        MeResponse,
        PostureResponse,
        Risk,
        RiskReport,
        ScoreTree,
    } from '#lib/types.js';
    import ChartLegend from '#lib/components/ChartLegend.svelte';
    import EChart from '#lib/components/EChart.svelte';
    import FanChart from '#lib/components/FanChart.svelte';
    import type { FanSeries } from '#lib/components/FanChart.svelte';

    let posture: PostureResponse | null = $state(null);
    let report: RiskReport | null = $state(null);
    let tree: ScoreTree | null = $state(null);
    let conformance: ConformanceReport | null = $state(null);
    const TREND_RANGE = '14d';
    const TREND_MS = 14 * 86_400_000;
    let error: string | null = $state(null);
    let me = $state<MeResponse | null>(null);
    let showSuppressed = $state(false);

    // Suppression form state: which risk, the reason and the expiry.
    let suppressing: string | null = $state(null);
    let reason = $state('');
    let days = $state(30);
    let actor = $state('');
    let submitting = $state(false);
    let formError: string | null = $state(null);

    const canSuppress = $derived(
        me ? !me.auth_required || (!!me.role && me.role !== 'viewer') : false,
    );
    const authOpen = $derived(me ? !me.auth_required : false);

    async function load() {
        error = null;
        try {
            [posture, report] = await Promise.all([api.posture(), api.risks()]);
        } catch (e) {
            error = String(e);
        }
        // The trend is decoration; its failure never hides the scan.
        api.scoreTree('', TREND_RANGE)
            .then((t) => (tree = t))
            .catch(() => (tree = null));
        api.otelConformance('24h')
            .then((c) => (conformance = c))
            .catch(() => (conformance = null));
    }

    $effect(() => {
        load();
        api.me()
            .then((m) => (me = m))
            .catch(() => (me = null));
    });

    function openSuppress(r: Risk) {
        suppressing = r.id;
        reason = '';
        days = 30;
        formError = null;
    }

    async function submitSuppress() {
        if (!suppressing) return;
        submitting = true;
        formError = null;
        try {
            await api.suppressRisk(
                suppressing,
                reason,
                days > 0 ? days : undefined,
                authOpen ? actor : undefined,
            );
            suppressing = null;
            await load();
        } catch (e) {
            formError = String(e);
        } finally {
            submitting = false;
        }
    }

    const SEVERITY_CLASS: Record<string, string> = {
        critical: 'fail',
        high: 'fail',
        medium: 'warn',
        low: 'neutral',
    };
    const bandClass = (b: string) => (b === 'good' ? 'ok' : b === 'fair' ? 'warn' : 'fail');

    // The charts are drawn in the colours of the theme in force.
    let theme = $state(chartTheme());
    onMount(() => followTheme((next) => (theme = next)));
    const t = $derived(chartTokens(theme));
    // The three states of a coverage cell in the order of their value, each
    // in the colour the heatmap and its legend draw it in.
    const coverageStates: LegendItem[] = $derived([
        { label: 'missing', color: t.fail },
        { label: 'stale', color: t.warn },
        { label: 'covered', color: t.ok },
    ]);
    const statusValue: Record<string, number> = $derived(
        Object.fromEntries(coverageStates.map((state, value) => [state.label, value])),
    );
    const axisName = $derived({ color: t.text });

    // Coverage heatmap: targets × disciplines, hue = covered / stale / missing.
    const heatmapOption: EChartsCoreOption | null = $derived.by(() => {
        if (!report) return null;
        const kinds = report.coverage.kinds;
        const targets = coverageLabels(report.coverage.targets);
        const data = report.coverage.targets.flatMap((c, y) =>
            c.cells.map((cell) => [kinds.indexOf(cell.kind), y, statusValue[cell.status] ?? 0]),
        );
        return {
            grid: { left: 150, right: 16, top: 26, bottom: 58 },
            tooltip: {
                formatter: tipFormatter((p: { data?: number[] }) => {
                    const [x, y, v] = p.data ?? [0, 0, 0];
                    const status = coverageStates[v]?.label ?? '';
                    return tip([strong(targets[y]), ` · ${disciplineLabel(kinds[x])}`], status);
                }),
            },
            xAxis: {
                type: 'category',
                data: kinds.map(disciplineLabel),
                name: 'discipline',
                nameLocation: 'middle',
                nameGap: 40,
                nameTextStyle: axisName,
                // Name every column; a bundled name goes on two lines
                // instead of being skipped or running into its neighbour.
                axisLabel: { interval: 0, formatter: stackedLabel },
            },
            yAxis: {
                type: 'category',
                data: targets,
                name: 'target',
                nameTextStyle: { ...axisName, align: 'right' },
            },
            visualMap: {
                show: false,
                min: 0,
                max: coverageStates.length - 1,
                inRange: { color: coverageStates.map((state) => state.color) },
            },
            series: [
                {
                    type: 'heatmap',
                    data,
                    itemStyle: { borderColor: 'transparent', borderWidth: 2 },
                },
            ],
        };
    });

    // Discipline coverage ratio across the portfolio.
    const radarOption: EChartsCoreOption | null = $derived.by(() => {
        if (!report) return null;
        const kinds = report.coverage.portfolio;
        return {
            tooltip: {},
            radar: {
                indicator: kinds.map((k) => ({ name: disciplineLabel(k.kind), max: 1 })),
                axisName: { color: t.text },
                splitLine: { lineStyle: { color: t.split } },
                splitArea: { show: false },
                axisLine: { lineStyle: { color: t.axis } },
            },
            series: [
                {
                    type: 'radar',
                    data: [{ value: kinds.map((k) => k.ratio), name: 'coverage' }],
                    areaStyle: { color: t.accent, opacity: 0.25 },
                    lineStyle: { color: t.accent },
                    itemStyle: { color: t.accent },
                },
            ],
        };
    });

    const pct = (v: number | null) => (v == null ? '–' : `${Math.round(v)}`);
    const LEVEL_CLASS: Record<string, string> = { spine: 'ok', target: 'warn', none: 'fail' };

    // What the cell of an objective with no margin counts as.
    const NO_MARGIN = { met: 0.5, breached: -0.5, other: 0 } as const;
    // The margin scale, low to high, each part in the colour the heatmap
    // and its legend draw it in.
    const marginScale: LegendItem[] = $derived([
        { label: 'margin on the bad side', color: t.fail },
        { label: 'margin near zero', color: t.warn },
        { label: 'margin on the good side', color: t.ok },
    ]);
    const nfrLegend: LegendItem[] = $derived([
        ...marginScale,
        {
            label: `no margin: met counts as ${NO_MARGIN.met}, breached as ${NO_MARGIN.breached}, any other verdict as ${NO_MARGIN.other}`,
        },
    ]);

    // NFR heatmap: targets × objectives, hue = margin (negative = breached).
    const nfrOption: EChartsCoreOption | null = $derived.by(() => {
        if (!posture || posture.nfrs.length === 0) return null;
        const ids = [...new Set(posture.nfrs.map((n) => n.nfr_id))];
        const targets = [...new Set(posture.nfrs.map((n) => n.target))];
        const cells = posture.nfrs;
        const value = (n: (typeof cells)[number]) =>
            n.margin ??
            (n.state === 'met'
                ? NO_MARGIN.met
                : n.state === 'breached'
                  ? NO_MARGIN.breached
                  : NO_MARGIN.other);
        const span = Math.max(1, ...cells.map((n) => Math.abs(value(n))));
        return {
            grid: { left: 110, right: 16, top: 26, bottom: 96 },
            tooltip: {
                formatter: tipFormatter((p: { data?: (number | string)[] }) => {
                    const i = Number(p.data?.[3] ?? -1);
                    const n = cells[i];
                    if (!n) return null;
                    const observed =
                        n.observed != null ? ` · observed ${n.observed} vs ${n.threshold}` : '';
                    const margin = n.margin != null ? ` · margin ${n.margin}` : '';
                    return tip(
                        [strong(n.target), ` · ${n.nfr_id} (${n.experiment})`],
                        `${n.state}${observed}${margin}`,
                    );
                }),
            },
            xAxis: {
                type: 'category',
                data: ids,
                name: 'objective',
                nameLocation: 'middle',
                nameGap: 78,
                nameTextStyle: axisName,
                // Every objective is a named column. An id is long and has
                // no place to break at, so the ids are written at a slant,
                // and a very long one is cut (the tooltip has all of it).
                axisLabel: {
                    interval: 0,
                    rotate: 40,
                    width: 96,
                    overflow: 'truncate',
                },
            },
            yAxis: {
                type: 'category',
                data: targets,
                name: 'target',
                nameTextStyle: { ...axisName, align: 'right' },
            },
            visualMap: {
                show: false,
                // The margin of a cell. Without this the scale reads the
                // last value of a cell, which is its place in the list.
                dimension: 2,
                min: -span,
                max: span,
                inRange: { color: marginScale.map((part) => part.color) },
            },
            series: [
                {
                    type: 'heatmap',
                    data: cells.map((n, i) => [
                        ids.indexOf(n.nfr_id),
                        targets.indexOf(n.target),
                        value(n),
                        i,
                    ]),
                    itemStyle: { borderColor: 'transparent', borderWidth: 2 },
                },
            ],
        };
    });

    // Score trend over the range: bucket indices become timestamps ending now.
    const trend: FanSeries | null = $derived.by(() =>
        tree ? sparkTrend(tree.sparkline, TREND_MS, Date.now()) : null,
    );
</script>

<div class="page-head">
    <h1>Resilience</h1>
    {#if report}
        <span class="muted">scanned {fmtAgo(report.as_of_ns)}</span>
    {/if}
</div>

{#if error}
    <div class="state error">{error}</div>
{:else if !report || !posture}
    <div class="state">Scanning…</div>
{:else}
    <div class="grid cols-2">
        <section class="panel" data-testid="risks">
            <h2>Top risks</h2>
            <div class="counts">
                <span class="badge fail">{report.summary.critical + report.summary.high} high+</span
                >
                <span class="badge warn">{report.summary.medium} medium</span>
                <span class="badge neutral">{report.summary.low} low</span>
                {#if report.summary.suppressed > 0}
                    <button class="link" onclick={() => (showSuppressed = !showSuppressed)}>
                        {report.summary.suppressed} suppressed
                    </button>
                {/if}
            </div>
            {#if report.risks.length === 0}
                <div class="state">No open findings.</div>
            {/if}
            <ol class="risks">
                {#each report.risks as r (r.id)}
                    <li>
                        <div class="row">
                            <span class="badge {SEVERITY_CLASS[r.severity] ?? 'neutral'}"
                                >{r.severity}</span
                            >
                            <span class="title">{r.title}</span>
                            {#if r.target}
                                <span class="mono target">{r.target}</span>
                            {/if}
                            <span class="mono rank" title="rank">{r.rank}</span>
                            {#if canSuppress}
                                <button class="link" onclick={() => openSuppress(r)}>accept</button>
                            {/if}
                        </div>
                        <div class="detail">{r.detail}</div>
                        {#if r.recommended_kind}
                            <div class="hint">
                                next: run a <b>{disciplineLabel(r.recommended_kind)}</b> test
                            </div>
                        {/if}
                        {#if suppressing === r.id}
                            <form
                                class="suppress"
                                onsubmit={(e) => {
                                    e.preventDefault();
                                    submitSuppress();
                                }}
                            >
                                <input
                                    placeholder="reason (why this is accepted)"
                                    bind:value={reason}
                                    required
                                    maxlength="500"
                                />
                                <label>
                                    days
                                    <input type="number" min="0" max="3650" bind:value={days} />
                                </label>
                                {#if authOpen}
                                    <input placeholder="your name" bind:value={actor} required />
                                {/if}
                                <button type="submit" disabled={submitting}>Accept</button>
                                <button
                                    type="button"
                                    class="link"
                                    onclick={() => (suppressing = null)}
                                >
                                    cancel
                                </button>
                                {#if formError}
                                    <span class="form-error">{formError}</span>
                                {/if}
                            </form>
                        {/if}
                    </li>
                {/each}
            </ol>
            {#if showSuppressed && report.suppressed.length > 0}
                <h2>Accepted for now</h2>
                <ol class="risks muted">
                    {#each report.suppressed as r (r.id)}
                        <li>
                            <div class="row">
                                <span class="badge neutral">{r.severity}</span>
                                <span class="title">{r.title}</span>
                                {#if r.target}<span class="mono target">{r.target}</span>{/if}
                            </div>
                            {#if r.suppression}
                                <div class="detail">
                                    {r.suppression.reason} — {r.suppression.actor},
                                    {r.suppression.expires_at_ns
                                        ? `until ${new Date(r.suppression.expires_at_ns / 1e6).toLocaleDateString()}`
                                        : 'no expiry'}
                                </div>
                            {/if}
                        </li>
                    {/each}
                </ol>
            {/if}
        </section>

        <section class="panel">
            <h2>Coverage gaps</h2>
            {#if report.coverage.targets.length === 0}
                <div class="state">No targets with evidence yet.</div>
            {:else}
                {#if heatmapOption}
                    <EChart
                        option={heatmapOption}
                        id="coverage"
                        label="Evidence coverage per target and discipline"
                        height={Math.max(160, 28 * report.coverage.targets.length + 94)}
                    />
                    <ChartLegend id="coverage" items={coverageStates} />
                {/if}
                {#if radarOption}
                    <EChart
                        option={radarOption}
                        id="coverage-radar"
                        height={240}
                        label="Evidence coverage per discipline: the share of targets covered, from 0 to 1"
                    />
                {/if}
            {/if}
        </section>
    </div>

    {#if nfrOption || trend}
        <div class="grid cols-2 below">
            {#if nfrOption}
                <section class="panel" data-testid="nfrs">
                    <h2>Objectives</h2>
                    <EChart
                        option={nfrOption}
                        id="nfr"
                        label="Objective verdicts per target"
                        height={Math.max(
                            160,
                            28 * new Set(posture.nfrs.map((n) => n.target)).size + 132,
                        )}
                    />
                    <ChartLegend id="nfr" items={nfrLegend} />
                </section>
            {/if}
            {#if trend}
                <section class="panel" data-testid="trend">
                    <h2>{COMPLIANCE_SCORE}, {TREND_RANGE}</h2>
                    <p class="muted meaning">{COMPLIANCE_SCORE_LINE}</p>
                    <FanChart
                        series={trend}
                        name={COMPLIANCE_SCORE}
                        unit="score"
                        id="trend"
                        height={220}
                        label="{COMPLIANCE_SCORE} over {TREND_RANGE}"
                    />
                </section>
            {/if}
        </div>
    {/if}

    {#if conformance && conformance.services.length > 0}
        <section class="panel below" data-testid="conformance">
            <h2>Telemetry contract, {conformance.range}</h2>
            <div class="counts">
                <span class="badge ok">{conformance.summary.spine} spine</span>
                <span class="badge warn">{conformance.summary.target} target</span>
                <span class="badge fail">{conformance.summary.none} none</span>
                <span class="muted">
                    services whose every span carried the target, the ICT spine, or neither
                </span>
            </div>
            <table>
                <thead>
                    <tr>
                        <th>Service</th>
                        <th>Spans</th>
                        <th>Level</th>
                        <th>Missing</th>
                    </tr>
                </thead>
                <tbody>
                    {#each conformance.services as svc (svc.service)}
                        <tr>
                            <td class="mono">{svc.service}</td>
                            <td class="mono">{svc.spans}</td>
                            <td
                                ><span class="badge {LEVEL_CLASS[svc.level] ?? 'neutral'}"
                                    >{svc.level}</span
                                ></td
                            >
                            <td class="mono missing">{svc.missing.join(', ') || '–'}</td>
                        </tr>
                    {/each}
                </tbody>
            </table>
            <div class="malformed" data-testid="malformed-ids">
                {#if (conformance.malformed_entity_ids ?? []).length === 0}
                    <span class="badge ok">0 malformed ict.entity.id</span>
                    <span class="muted">
                        every id received matches
                        <span class="mono"
                            >{conformance.entity_id_pattern ?? 'the canonical form'}</span
                        >
                    </span>
                {:else}
                    <span class="badge fail">
                        {conformance.malformed_entity_ids
                            .length}{conformance.malformed_entity_ids_truncated ? '+' : ''} malformed
                        ict.entity.id
                    </span>
                    <span class="muted">
                        stored as received; fix them at the sender (expected
                        <span class="mono">{conformance.entity_id_pattern}</span>, e.g. cif-0042)
                    </span>
                    <ul>
                        {#each conformance.malformed_entity_ids as bad (bad.service + bad.entity_id)}
                            <li>
                                <span class="mono">{bad.entity_id}{bad.truncated ? '…' : ''}</span>
                                from
                                <span class="mono">{bad.service}</span>, {bad.spans} spans:
                                {bad.reason}
                            </li>
                        {/each}
                    </ul>
                {/if}
            </div>
        </section>
    {/if}

    <section class="panel score" data-testid="posture">
        <h2>{RESILIENCE_SCORE}</h2>
        <p class="muted meaning">{RESILIENCE_SCORE_LINE}</p>
        <div class="headline">
            <span class="value mono">{posture.posture.score.toFixed(1)}</span>
            <span class="badge {bandClass(posture.posture.band)}">{posture.posture.band}</span>
        </div>
        {#if posture.posture.targets.length > 0}
            <table>
                <thead>
                    <tr>
                        <th>Target</th>
                        <th>Env</th>
                        <th>Score</th>
                        <th>NFR</th>
                        <th>Coverage</th>
                        <th>Freshness</th>
                        <th>Recovery</th>
                    </tr>
                </thead>
                <tbody>
                    {#each posture.posture.targets as tp (tp.target + (tp.environment ?? ''))}
                        <tr>
                            <td class="mono">{tp.target}</td>
                            <td class="mono">{tp.environment ?? '–'}</td>
                            <td
                                ><span class="badge {bandClass(tp.band)}"
                                    >{tp.score.toFixed(1)}</span
                                ></td
                            >
                            <td class="mono">{pct(tp.components.nfr)}</td>
                            <td class="mono">{pct(tp.components.coverage)}</td>
                            <td class="mono">{pct(tp.components.freshness)}</td>
                            <td class="mono">{pct(tp.components.recovery)}</td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        {/if}
    </section>
{/if}

<style>
    .muted {
        color: var(--text-dim);
        font-size: var(--font-size-sm);
    }
    .meaning {
        margin: 0 0 var(--space-2-5);
    }
    .counts {
        display: flex;
        gap: var(--space-2);
        align-items: center;
        margin-bottom: var(--space-2-5);
    }
    .risks {
        list-style: none;
        margin: 0;
        padding: 0;
    }
    .risks li {
        padding: 9px 0;
        border-top: var(--border-width-sm) solid var(--border);
    }
    .risks .row {
        display: flex;
        align-items: center;
        gap: var(--space-2);
        min-width: 0;
    }
    .risks .title {
        font-weight: var(--font-weight-strong);
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .risks .target,
    .risks .rank {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
    }
    .risks .detail {
        color: var(--text-dim);
        font-size: var(--font-size-sm);
        margin-top: 3px;
    }
    .risks .hint {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
        margin-top: var(--space-0-5);
    }
    .link {
        background: none;
        border: none;
        color: var(--accent);
        cursor: pointer;
        padding: 0;
        font: inherit;
        font-size: var(--font-size-sm);
    }
    .suppress {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2);
        align-items: center;
        margin-top: var(--space-2);
    }
    .suppress input {
        background: var(--bg-raised);
        color: var(--text);
        border: var(--border-width-sm) solid var(--border-strong);
        border-radius: var(--radius-md);
        padding: var(--space-1) var(--space-2);
        font: inherit;
    }
    .suppress input[type='number'] {
        width: 64px;
    }
    .suppress label {
        display: flex;
        gap: var(--space-1-5);
        align-items: center;
        color: var(--text-dim);
        font-size: var(--font-size-sm);
    }
    .suppress button[type='submit'] {
        background: var(--accent-dim);
        color: var(--text);
        border: none;
        border-radius: var(--radius-md);
        padding: var(--space-1) var(--space-3);
        cursor: pointer;
    }
    .missing {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
    }
    .form-error {
        color: var(--fail);
        font-size: var(--font-size-sm);
    }
    .score,
    .below {
        margin-top: var(--space-3-5);
    }
    .headline {
        display: flex;
        align-items: baseline;
        gap: var(--space-3);
        margin-bottom: var(--space-3);
    }
    .headline .value {
        font-size: 34px;
        font-weight: var(--font-weight-strong);
    }
    table {
        width: 100%;
        border-collapse: collapse;
        font-size: var(--font-size-md);
    }
    th {
        text-align: left;
        color: var(--text-dim);
        font-weight: var(--font-weight-strong);
        font-size: var(--font-size-2xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-sm);
        padding: var(--space-1-5) var(--space-2);
        border-bottom: var(--border-width-sm) solid var(--border);
    }
    td {
        padding: var(--space-1-5) var(--space-2);
        border-bottom: var(--border-width-sm) solid var(--border);
    }
    .malformed {
        margin-top: var(--space-2-5);
        font-size: var(--font-size-sm);
    }
    .malformed ul {
        margin: var(--space-1-5) 0 0;
        padding-left: var(--space-4-5);
    }
</style>

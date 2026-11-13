<script lang="ts">
    // Posture: the executive opening view. Ranked risks and discipline
    // coverage gaps first (the passive scan runs with no faults), then the
    // continuous posture score per target. Operators can accept a finding
    // for a while; suppressions are hash-chained server side.
    import { api, fmtAgo } from '#lib/api.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import { chartTokens } from '#lib/echarts.js';
    import type { MeResponse, PostureResponse, Risk, RiskReport } from '#lib/types.js';
    import EChart from '#lib/components/EChart.svelte';

    let posture: PostureResponse | null = $state(null);
    let report: RiskReport | null = $state(null);
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

    const t = chartTokens();
    const STATUS_VALUE: Record<string, number> = { missing: 0, stale: 1, covered: 2 };

    // Coverage heatmap: targets × disciplines, hue = covered / stale / missing.
    const heatmapOption: EChartsCoreOption | null = $derived.by(() => {
        if (!report) return null;
        const kinds = report.coverage.kinds;
        const targets = report.coverage.targets.map((c) => c.target);
        const data = report.coverage.targets.flatMap((c, y) =>
            c.cells.map((cell) => [kinds.indexOf(cell.kind), y, STATUS_VALUE[cell.status] ?? 0]),
        );
        return {
            grid: { left: 110, right: 16, top: 10, bottom: 40 },
            tooltip: {
                ...t.tooltip,
                formatter: (p: { data?: number[] }) => {
                    const [x, y, v] = p.data ?? [0, 0, 0];
                    const status = ['missing', 'stale', 'covered'][v] ?? '';
                    return `<b>${targets[y]}</b> · ${kinds[x]}<br/>${status}`;
                },
            },
            xAxis: { type: 'category', data: kinds, axisLabel: { color: t.text } },
            yAxis: { type: 'category', data: targets, axisLabel: { color: t.text } },
            visualMap: {
                show: false,
                min: 0,
                max: 2,
                inRange: { color: [t.fail, t.warn, t.ok] },
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
            tooltip: { ...t.tooltip },
            radar: {
                indicator: kinds.map((k) => ({ name: k.kind, max: 1 })),
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
</script>

<div class="page-head">
    <h1>Posture</h1>
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
                            <div class="hint">next: run a <b>{r.recommended_kind}</b> test</div>
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
                        height={Math.max(160, 28 * report.coverage.targets.length + 60)}
                    />
                {/if}
                {#if radarOption}
                    <EChart option={radarOption} id="coverage-radar" height={240} />
                {/if}
            {/if}
        </section>
    </div>

    <section class="panel score" data-testid="posture">
        <h2>Posture score</h2>
        <div class="headline">
            <span class="value mono">{posture.posture.score.toFixed(1)}</span>
            <span class="badge {bandClass(posture.posture.band)}">{posture.posture.band}</span>
            <span class="muted">
                continuous score: objective margins, discipline coverage, evidence freshness and
                recovery against target, per target and criticality weighted
            </span>
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
        font-size: 12.5px;
    }
    .counts {
        display: flex;
        gap: 8px;
        align-items: center;
        margin-bottom: 10px;
    }
    .risks {
        list-style: none;
        margin: 0;
        padding: 0;
    }
    .risks li {
        padding: 9px 0;
        border-top: 1px solid var(--border);
    }
    .risks .row {
        display: flex;
        align-items: center;
        gap: 8px;
        min-width: 0;
    }
    .risks .title {
        font-weight: 600;
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .risks .target,
    .risks .rank {
        color: var(--text-dim);
        font-size: 12px;
    }
    .risks .detail {
        color: var(--text-dim);
        font-size: 12.5px;
        margin-top: 3px;
    }
    .risks .hint {
        color: var(--text-faint);
        font-size: 12px;
        margin-top: 2px;
    }
    .link {
        background: none;
        border: none;
        color: var(--accent);
        cursor: pointer;
        padding: 0;
        font: inherit;
        font-size: 12.5px;
    }
    .suppress {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        align-items: center;
        margin-top: 8px;
    }
    .suppress input {
        background: var(--bg-raised);
        color: var(--text);
        border: 1px solid var(--border-strong);
        border-radius: 4px;
        padding: 4px 8px;
        font: inherit;
    }
    .suppress input[type='number'] {
        width: 64px;
    }
    .suppress label {
        display: flex;
        gap: 6px;
        align-items: center;
        color: var(--text-dim);
        font-size: 12.5px;
    }
    .suppress button[type='submit'] {
        background: var(--accent-dim);
        color: var(--text);
        border: none;
        border-radius: 4px;
        padding: 4px 12px;
        cursor: pointer;
    }
    .form-error {
        color: var(--fail);
        font-size: 12.5px;
    }
    .score {
        margin-top: 14px;
    }
    .headline {
        display: flex;
        align-items: baseline;
        gap: 12px;
        margin-bottom: 12px;
    }
    .headline .value {
        font-size: 34px;
        font-weight: 600;
    }
    table {
        width: 100%;
        border-collapse: collapse;
        font-size: 13px;
    }
    th {
        text-align: left;
        color: var(--text-dim);
        font-weight: 600;
        font-size: 11.5px;
        text-transform: uppercase;
        letter-spacing: 0.5px;
        padding: 6px 8px;
        border-bottom: 1px solid var(--border);
    }
    td {
        padding: 6px 8px;
        border-bottom: 1px solid var(--border);
    }
</style>

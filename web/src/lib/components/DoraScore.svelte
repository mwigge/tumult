<script lang="ts">
    // DORA coverage and the internal maturity level of one legal entity:
    // header, articles × functions heatmap, expectation table, maturity
    // with its gates and the per-node roll-up, the plan scenarios of each
    // critical or important function, reporting readiness.
    // Every number is shown with its unknown share.
    import { onMount } from 'svelte';
    import { api } from '#lib/api.js';
    import { chartTheme, followTheme } from '#lib/echarts.js';
    import type { ChartTheme } from '#lib/echarts.js';
    import {
        COMPONENTS,
        capText,
        cellLines,
        cellOf,
        componentsLegend,
        componentsOption,
        coverageText,
        durationText,
        gaugeOption,
        heatmapHeight,
        heatmapLegend,
        heatmapOption,
        historyLegend,
        historyOption,
        historyStateText,
        nodeTree,
        pct,
        unknownLabel,
    } from '#lib/dora.js';
    import type { HeatmapMode } from '#lib/dora.js';
    import type { DoraExpectation, DoraReport, MaturityReport } from '#lib/types.js';
    import Badge from './Badge.svelte';
    import ChartLegend from './ChartLegend.svelte';
    import DoraEvidence from './DoraEvidence.svelte';
    import DoraNotices from './DoraNotices.svelte';
    import DoraScenarios from './DoraScenarios.svelte';
    import EChart from './EChart.svelte';
    import ScoreNumber from './ScoreNumber.svelte';
    import Panel from './Panel.svelte';
    import Segmented from './Segmented.svelte';
    import State from './State.svelte';

    let { lei, onlei }: { lei: string; onlei: (lei: string) => void } = $props();

    let report = $state<DoraReport | null>(null);
    let maturity = $state<MaturityReport | null>(null);
    let error = $state<string | null>(null);
    let mode = $state<HeatmapMode>('coverage');
    const HEATMAP_VALUES: { value: HeatmapMode; label: string }[] = [
        { value: 'coverage', label: 'Coverage' },
        { value: 'status', label: 'Evidence status' },
    ];
    /** The row whose evidence the drawer shows. */
    let opened = $state<DoraExpectation | null>(null);

    let theme = $state<ChartTheme>(chartTheme());
    onMount(() => followTheme((next) => (theme = next)));

    $effect(() => {
        const selected = lei;
        let cancelled = false;
        error = null;
        opened = null;
        Promise.all([api.dora(selected), api.maturity(selected, 'node')])
            .then(([d, m]) => {
                if (cancelled) return;
                report = d;
                maturity = m;
            })
            .catch((e) => !cancelled && (error = String(e)));
        return () => {
            cancelled = true;
        };
    });

    const heatmap = $derived(report ? heatmapOption(report, mode, theme) : null);
    const gauge = $derived(maturity ? gaugeOption(maturity, theme) : null);
    const components = $derived(maturity ? componentsOption(maturity, theme) : null);
    const history = $derived(maturity ? historyOption(maturity, theme) : null);
    const expectations = $derived(
        report ? [...report.expectations].sort((a, b) => b.priority - a.priority) : [],
    );
    const tree = $derived(maturity ? nodeTree(maturity.nodes) : []);
    // The roll-up headline, or what is true when there is none: only an
    // empty table means there are no nodes.
    const rollupSuffix = $derived.by(() => {
        if (maturity?.rollup_headline) return ` — ${maturity.rollup_headline}`;
        if (tree.length === 0) return ' — no nodes on record';
        return '';
    });

    const date = (ns: number | null) =>
        ns === null ? '–' : new Date(ns / 1e6).toISOString().slice(0, 10);

    /** Share of the requirement rows whose standing is unknown. */
    function statusUnknown(r: DoraReport): number {
        const judged = r.status.meets + r.status.gap + r.status.unknown;
        return judged === 0 ? 1 : r.status.unknown / judged;
    }

    /** Share of the functions the register says nothing about. */
    function unregistered(r: DoraReport): number {
        const { functions, registered } = r.readiness;
        return functions === 0 ? 1 : (functions - registered) / functions;
    }
</script>

{#if error}
    <Panel><State kind="error">Failed to load the DORA score: {error}</State></Panel>
{:else if !report || !maturity}
    <Panel><State kind="loading" height={320} /></Panel>
{:else}
    <div data-testid="dora-view">
        <section class="panel head" data-testid="dora-header">
            <div class="who">
                <label>
                    <span class="lbl">Legal entity</span>
                    <select
                        aria-label="Legal entity"
                        value={report.lei ?? ''}
                        onchange={(e) => onlei(e.currentTarget.value)}
                        disabled={report.legal_entities.length < 2}
                    >
                        {#if report.legal_entities.length === 0}
                            <option value="">no legal entity registered</option>
                        {/if}
                        {#each report.legal_entities as entity (entity.lei)}
                            <option value={entity.lei}>{entity.name} · {entity.lei}</option>
                        {/each}
                    </select>
                </label>
                <span class="name">{report.entity_name ?? 'unregistered entity'}</span>
                <Badge tone="neutral" data-testid="profile" label={report.profile_label} />
                {#each report.profile_flags as flag (flag)}
                    <Badge tone="neutral" label={flag} />
                {/each}
            </div>
            <div class="headline" class:capped={maturity.capped}>{maturity.headline}</div>
            {#if maturity.cap.reason}
                <div class="cap" data-testid="cap-reason">{maturity.cap.reason}</div>
            {/if}
            <div class="note">
                {maturity.scale.source} Requirement registry {report.registry_version}, as of {date(
                    report.as_of_ns,
                )}.
            </div>
            <DoraNotices doc={report} />
        </section>

        <div class="tiles">
            <ScoreNumber
                label="Maturity level"
                value="{maturity.level} / {maturity.target}"
                unknown={maturity.unknown_share}
                hint="{maturity.scale.levels[maturity.level - 1] ??
                    ''} · level of target, internal scale"
            />
            <ScoreNumber
                label="Maturity index"
                value={maturity.index.toFixed(2)}
                unknown={maturity.unknown_share}
                muted={maturity.capped}
                hint="{maturity.capped
                    ? 'greyed: a cap decides the level, not the index'
                    : 'rolled up over every node, 1–5 scale'} · pillars alone {maturity.flat_index.toFixed(
                    2,
                )}"
            />
            <ScoreNumber
                label="DORA coverage"
                value={coverageText(report.portfolio.coverage)}
                unknown={report.portfolio.unknown}
                hint="criticality-weighted over all applicable rows"
            />
            <ScoreNumber
                label="Rows meeting expectation"
                value="{report.status.meets} / {report.status.meets + report.status.gap}"
                unknown={statusUnknown(report)}
                hint="{report.status.no_events} without events · {report.status.na} not applicable"
            />
        </div>

        <section class="panel" data-testid="dora-heatmap" data-scroll>
            <div class="panel-head">
                <h2>Articles × {report.by === 'org' ? 'org units' : 'functions'}</h2>
                <Segmented
                    label="heatmap value"
                    class="toggle"
                    selectedClass="on"
                    options={HEATMAP_VALUES}
                    value={mode}
                    onchange={(next) => (mode = next)}
                />
            </div>
            {#if report.rows.length === 0 || report.articles.length === 0}
                <State kind="empty">
                    No functions on record. Declare them under <span class="mono">functions:</span> in
                    org.yaml.
                </State>
            {:else if heatmap}
                <EChart
                    option={heatmap}
                    height={heatmapHeight(report.rows.length)}
                    id="dora-heatmap"
                    label="Heatmap of {mode === 'coverage'
                        ? 'coverage'
                        : 'evidence status'} per article and {report.by === 'org'
                        ? 'org unit'
                        : 'function'}. The same values follow as a table."
                />
                <details class="alt">
                    <summary>Show the heatmap as a table</summary>
                    <table class="data" aria-label="Articles by function, as a table">
                        <thead>
                            <tr>
                                <th scope="col">{report.by === 'org' ? 'Org unit' : 'Function'}</th>
                                {#each report.articles as a (a.article)}
                                    <th scope="col">Art. {a.article}</th>
                                {/each}
                            </tr>
                        </thead>
                        <tbody>
                            {#each report.rows as row (row.entity_id)}
                                <tr>
                                    <th scope="row">{row.label}</th>
                                    {#each report.articles as a (a.article)}
                                        {@const cell = cellOf(report, a.article, row.entity_id)}
                                        <td>
                                            {#if cell}
                                                {#each cellLines(cell) as line (line)}
                                                    <div>{line}</div>
                                                {/each}
                                            {:else}
                                                not applicable
                                            {/if}
                                        </td>
                                    {/each}
                                </tr>
                            {/each}
                        </tbody>
                    </table>
                </details>
            {/if}
            <ChartLegend id="dora-heatmap" items={heatmapLegend(mode)} />
            <table class="data articles">
                <thead>
                    <tr><th>Article</th><th>Coverage</th><th>Meets</th><th>Gap</th></tr>
                </thead>
                <tbody>
                    {#each report.articles as a (a.article)}
                        <tr>
                            <td>Art. {a.article} <span class="dim">{a.title}</span></td>
                            <td data-figure>
                                <span class="mono">{coverageText(a.coverage)}</span>
                                <span class="unk">{unknownLabel(a.unknown)}</span>
                            </td>
                            <td class="mono">{a.status?.meets ?? '–'}</td>
                            <td class="mono">{a.status?.gap ?? '–'}</td>
                        </tr>
                    {/each}
                </tbody>
            </table>
            <div class="foot">
                {report.na.count} rows not applicable to profile {report.profile_label}
            </div>
        </section>

        <section class="panel" data-testid="dora-expectations" data-scroll>
            <h2>Expected and actual evidence status, largest gap first</h2>
            {#if expectations.length === 0}
                <State kind="empty">No requirement applies to this profile.</State>
            {:else}
                <table class="data">
                    <thead>
                        <tr>
                            <th>Requirement</th><th>Expected</th><th>Actual</th><th>Gap</th>
                            <th>Oldest evidence</th><th>Days until operating</th><th>Evidence</th>
                        </tr>
                    </thead>
                    <tbody>
                        {#each expectations as e (e.id)}
                            <tr>
                                <td>
                                    <span class="mono">{e.cite}</span>
                                    {e.title}
                                    {#if e.scope_basis === 'house'}
                                        <Badge
                                            tone="neutral"
                                            data-testid="house-scope"
                                            title="The regulation does not limit this row to these functions; the house does."
                                            label="house scope"
                                        />
                                    {/if}
                                    <div class="dim mono">{e.id}</div>
                                    {#if e.reason}<div class="dim">{e.reason}</div>{/if}
                                </td>
                                <td>{e.expected}</td>
                                <td>{e.actual.replace('_', ' ')}</td>
                                <td>
                                    <Badge
                                        kind="standing"
                                        value={e.standing}
                                        label={e.standing === 'gap'
                                            ? `−${e.gap}`
                                            : e.standing.replace('_', ' ')}
                                    />
                                    {#if e.short.length > 0}
                                        <div class="dim mono">{e.short.join(', ')}</div>
                                    {/if}
                                </td>
                                <td class="mono">{e.oldest_evidence_date ?? '–'}</td>
                                <td class="mono">{e.days_until_operating ?? '–'}</td>
                                <td>
                                    {#if e.evidence}
                                        <button
                                            class="link mono"
                                            aria-haspopup="dialog"
                                            title="{e.evidence.source} evidence behind {e.cite}"
                                            onclick={() => (opened = e)}>{e.evidence.id}</button
                                        >
                                    {:else}–{/if}
                                </td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            {/if}
            {#if report.disclaimer}<div class="foot">{report.disclaimer}</div>{/if}
        </section>

        {#if report.scenarios}
            <DoraScenarios scenarios={report.scenarios} />
        {/if}

        <section class="panel" data-testid="dora-maturity" data-scroll>
            <h2>Maturity — {maturity.headline}</h2>
            <div class="maturity">
                <div class="gauge">
                    {#if gauge}
                        <EChart
                            option={gauge}
                            height={210}
                            id="dora-gauge"
                            label="Gauge: maturity index {maturity.index.toFixed(
                                2,
                            )} on the internal 1 to 5 scale, {maturity.headline}"
                        />
                    {/if}
                    <div class="dim center">
                        index on the internal 1–5 scale · {unknownLabel(maturity.unknown_share)}
                    </div>
                    <div class="dim center">
                        pillars alone: {unknownLabel(maturity.flat_unknown_share)}
                    </div>
                </div>
                <div class="bars">
                    {#if components}
                        <EChart
                            option={components}
                            height={210}
                            id="dora-components"
                            label="Bars: {COMPONENTS.map(
                                (c) =>
                                    `${c.label} ${maturity?.components[c.key]?.toFixed(1) ?? 'unknown'}`,
                            ).join(', ')}"
                        />
                        <ChartLegend id="dora-components" items={componentsLegend(theme)} />
                    {/if}
                    <ul class="reasons">
                        {#each COMPONENTS as c (c.key)}
                            {@const state = maturity.component_states[c.key]}
                            {#if state?.reason}
                                <li><b>{c.label}</b>: {state.reason}</li>
                            {/if}
                        {/each}
                    </ul>
                </div>
            </div>
            <div class="maturity">
                <div class="history" data-testid="dora-history">
                    {#if history && maturity.history.length > 0}
                        <EChart
                            option={history}
                            height={170}
                            id="dora-history"
                            label="Line chart of level and index per quarter. The same values follow as a table."
                        />
                        <ChartLegend id="dora-history" items={historyLegend(theme)} />
                        <div class="dim center">
                            level and index per quarter, from the days the daily snapshot stored
                        </div>
                        <table class="data">
                            <thead>
                                <tr><th>Quarter</th><th>From</th><th>Level</th><th>Index</th></tr>
                            </thead>
                            <tbody>
                                {#each maturity.history as h (h.quarter)}
                                    <tr>
                                        <td class="mono">{h.quarter}</td>
                                        <td>
                                            {historyStateText(h)}
                                            {#if h.reason}<div class="dim">{h.reason}</div>{/if}
                                        </td>
                                        <td class="mono">{h.level ?? '–'}</td>
                                        {#if h.index === null}
                                            <td class="mono">–</td>
                                        {:else}
                                            <td data-figure>
                                                <span class="mono">{h.index.toFixed(2)}</span>
                                                <span class="unk"
                                                    >{unknownLabel(h.unknown_share ?? 1)}</span
                                                >
                                            </td>
                                        {/if}
                                    </tr>
                                {/each}
                            </tbody>
                        </table>
                    {/if}
                </div>
                <table class="data gates">
                    <thead><tr><th>Gate</th><th>State</th><th>Failing conditions</th></tr></thead>
                    <tbody>
                        {#each maturity.gates as g (g.level)}
                            <tr>
                                <td
                                    >Level {g.level}
                                    <span class="dim"
                                        >{maturity.scale.levels[g.level - 1] ?? ''}</span
                                    ></td
                                >
                                <td>
                                    <Badge kind="verdict" value={g.passed ? 'passed' : 'blocked'} />
                                </td>
                                <td>{g.failing.join('; ') || '–'}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            </div>
        </section>

        <section class="panel" data-testid="dora-rollup" data-scroll>
            <h2>Maturity per node{rollupSuffix}</h2>
            {#if tree.length === 0}
                <State kind="empty">No nodes to roll up.</State>
            {:else}
                <table class="data">
                    <thead>
                        <tr><th>Node</th><th>Level</th><th>Index</th><th>Cap</th></tr>
                    </thead>
                    <tbody>
                        {#each tree as { node, depth } (node.id)}
                            <tr>
                                <td style="padding-left: {10 + depth * 18}px">
                                    <span class="mono">{node.id}</span>
                                    <span class="dim">{node.kind}</span>
                                    {#if node.critical}<Badge tone="warn" label="critical" />{/if}
                                    {#if node.unknown}
                                        <Badge tone="neutral" label="not instrumented" />
                                    {/if}
                                    {#if node.note}<div class="dim">{node.note}</div>{/if}
                                </td>
                                <td class="mono">{node.level} / {node.target}</td>
                                <td data-figure>
                                    <span class="mono">{node.index.toFixed(2)}</span>
                                    <span class="unk">{unknownLabel(node.unknown_share)}</span>
                                </td>
                                <td>{node.cap.rule === 'none' ? '–' : capText(node)}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            {/if}
            <div class="foot">
                A node is the criticality-weighted mean of its children, capped by its weakest
                critical child. A node that sends nothing counts at the floor.
            </div>
        </section>

        <section class="panel" data-testid="dora-readiness" data-scroll>
            <h2>Reporting readiness — function register</h2>
            <div class="tiles">
                <ScoreNumber
                    label="Functions with RTO"
                    value="{report.readiness.with_rto} / {report.readiness.functions}"
                    unknown={unregistered(report)}
                />
                <ScoreNumber
                    label="Functions with RPO"
                    value="{report.readiness.with_rpo} / {report.readiness.functions}"
                    unknown={unregistered(report)}
                />
                <ScoreNumber
                    label="With assessment date"
                    value="{report.readiness.with_assessment} / {report.readiness.functions}"
                    unknown={unregistered(report)}
                />
                <ScoreNumber
                    label="Complete rows"
                    value="{report.readiness.complete} / {report.readiness.functions}"
                    unknown={unregistered(report)}
                    hint="RTO, RPO and assessment date all present"
                />
            </div>
            <table class="data functions">
                <thead>
                    <tr>
                        <th>Function</th><th>Code</th><th>Tier</th><th>RTO</th><th>RPO</th>
                        <th>Assessed</th><th>Coverage</th>
                    </tr>
                </thead>
                <tbody>
                    {#each report.functions as f (f.entity_id)}
                        <tr>
                            <td>
                                <span class="mono">{f.entity_id}</span>
                                {f.name}
                                {#if !f.registered}
                                    <Badge tone="warn" label="not in register" />
                                {/if}
                            </td>
                            <td class="mono">{f.register_codes.join(', ') || '–'}</td>
                            <td>{f.tier}</td>
                            <td class="mono">{durationText(f.rto_s)}</td>
                            <td class="mono">{durationText(f.rpo_s)}</td>
                            <td class="mono">{f.last_assessed ?? '–'}</td>
                            <td data-figure>
                                <span class="mono">{coverageText(f.coverage)}</span>
                                <span class="unk">{unknownLabel(f.unknown)}</span>
                            </td>
                        </tr>
                    {/each}
                </tbody>
            </table>
            <dl class="register">
                <dt>Register of information</dt>
                <dd>
                    {#if report.readiness.register.connected}
                        connected, {report.readiness.register.rows} rows, reconciled {date(
                            report.readiness.register.reconciled_ns,
                        )}
                    {:else}
                        not connected: Art. 28 rows are unknown
                    {/if}
                </dd>
                <dt>Signed off</dt>
                <dd>
                    {#if report.readiness.register.signed_by}
                        {report.readiness.register.signed_by}
                        {report.readiness.register.signed_at ?? ''}
                    {:else}
                        no sign-off recorded
                    {/if}
                </dd>
                <dt>Services mapped to no function</dt>
                <dd class="mono">
                    {report.readiness.unmapped_services.join(', ') || 'none'}
                </dd>
            </dl>
            <div class="foot">
                {pct(1 - unregistered(report))} of the functions seen are in the register.
            </div>
        </section>
    </div>
    {#if opened}
        <DoraEvidence row={opened} lei={report.lei ?? ''} onclose={() => (opened = null)} />
    {/if}
{/if}

<style>
    .head .who {
        display: flex;
        align-items: center;
        gap: var(--space-2-5);
        flex-wrap: wrap;
    }
    .head .lbl {
        color: var(--text-dim);
        font-size: var(--font-size-2xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-md);
        margin-right: var(--space-1-5);
    }
    .head select {
        background: var(--bg-raised);
        color: var(--text);
        border: var(--border-width-sm) solid var(--border-strong);
        border-radius: var(--radius-md);
        padding: 3px var(--space-1-5);
        font: inherit;
        font-size: var(--font-size-sm);
    }
    .head .name {
        font-weight: var(--font-weight-strong);
    }
    .headline {
        font-size: var(--font-size-2xl);
        font-weight: var(--font-weight-strong);
        margin-top: var(--space-2-5);
    }
    .headline.capped {
        color: var(--warn);
    }
    .cap {
        margin-top: var(--space-1);
        color: var(--text);
    }
    .note,
    .foot,
    .dim {
        color: var(--text-faint);
        font-size: var(--font-size-2xs);
    }
    .note,
    .foot {
        margin-top: var(--space-2);
    }
    .center {
        text-align: center;
    }
    .tiles {
        display: grid;
        grid-template-columns: repeat(4, 1fr);
        gap: var(--space-3);
        margin: var(--space-3) 0;
    }
    section.panel {
        margin-bottom: var(--space-3);
    }
    .panel-head {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: var(--space-3);
    }
    .link {
        background: none;
        border: none;
        padding: 0;
        color: var(--accent);
        font-size: inherit;
        cursor: pointer;
        text-align: left;
    }
    .link:hover {
        text-decoration: underline;
    }
    .alt {
        margin: var(--space-1) 0 var(--space-2-5);
        font-size: var(--font-size-sm);
    }
    .alt summary {
        cursor: pointer;
        color: var(--text-dim);
    }
    .unk {
        color: var(--text-faint);
        font-size: var(--font-size-3xs);
        margin-left: var(--space-1-5);
        white-space: nowrap;
    }
    .maturity {
        display: grid;
        grid-template-columns: minmax(220px, 1fr) 2fr;
        gap: var(--space-4);
        align-items: start;
        margin-bottom: var(--space-3);
    }
    .reasons {
        margin: var(--space-1) 0 0;
        padding-left: var(--space-4-5);
        color: var(--text-dim);
        font-size: var(--font-size-xs);
    }
    .register {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: var(--space-1) var(--space-4);
        margin: var(--space-3) 0 0;
        font-size: var(--font-size-sm);
    }
    .register dt {
        color: var(--text-dim);
    }
    .register dd {
        margin: 0;
    }
    @media (max-width: 900px) {
        .tiles {
            grid-template-columns: repeat(2, 1fr);
        }
        .maturity {
            grid-template-columns: 1fr;
        }
    }
</style>

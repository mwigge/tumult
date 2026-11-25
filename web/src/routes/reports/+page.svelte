<script lang="ts">
    import { onMount } from 'svelte';
    import { api } from '#lib/api.js';
    import { DEFAULT_PERIOD, PERIOD_DAYS, PERIOD_LABEL, localDay, viewQuery } from '#lib/dr.js';
    import { templateCode } from '#lib/report-codes.js';
    import Badge from '#lib/components/Badge.svelte';
    import type {
        DrTarget,
        ExperimentRow,
        ManualExperiment,
        MeResponse,
        MetricDefInfo,
        ReportFile,
        ReportMetaV2,
        ReportTemplate,
    } from '#lib/types.js';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';

    let me = $state<MeResponse | null>(null);
    const canGenerate = $derived(
        !!me && (!me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer')),
    );

    // ---- v2: compliance-grade reports --------------------------------------
    let reportsV2: ReportMetaV2[] | null = $state(null);
    let errorV2: string | null = $state(null);

    let template: ReportTemplate = $state('executive-digest');
    let period = $state('7d');
    let framework = $state('dora');
    let experimentId = $state('');
    let experiments: ExperimentRow[] = $state([]);

    let generatingV2 = $state(false);
    let generateErrorV2: string | null = $state(null);
    let preview: string | null = $state(null);

    const templates: { id: ReportTemplate; label: string; hint: string }[] = [
        {
            id: 'executive-digest',
            label: 'R1 · Executive digest',
            hint: 'portfolio score, trends, decisions',
        },
        {
            id: 'game-day',
            label: 'R3 · Game-day report',
            hint: 'one experiment run, timeline + verdict',
        },
        {
            id: 'evidence-pack',
            label: 'R2 · Evidence pack',
            hint: 'auditor traceability per framework',
        },
        {
            id: 'dr-test',
            label: 'R4 · DR test report',
            hint: 'the DR tests of one target over a period',
        },
        {
            id: 'dr-exercise',
            label: 'R5 · DR exercise report',
            hint: 'one manual DR exercise',
        },
    ];

    // The DR reports: R4 is of a target of the DR register over a period,
    // R5 of one submitted or verified manual record. Their choices load
    // when their template is picked.
    let drTargets: DrTarget[] | null = $state(null);
    let targetIndex = $state(0);
    let drPeriod = $state(DEFAULT_PERIOD);
    let drRecords: ManualExperiment[] | null = $state(null);
    let recordId = $state('');

    const targetName = (t: DrTarget) =>
        t.environment ? `${t.system} · ${t.environment}` : t.system;

    $effect(() => {
        if (template === 'dr-test' && drTargets === null) {
            api.dr()
                .then((view) => (drTargets = view.targets))
                .catch((e) => (generateErrorV2 = String(e)));
        }
        if (template === 'dr-exercise' && drRecords === null) {
            api.manualList('')
                .then((r) => {
                    drRecords = r.records.filter(
                        (rec) => rec.status === 'submitted' || rec.status === 'verified',
                    );
                    recordId = drRecords[0]?.id ?? '';
                })
                .catch((e) => (generateErrorV2 = String(e)));
        }
    });

    /** Whether the template in force has what its report needs. */
    function hasChoices(): boolean {
        if (template === 'game-day') return experimentId !== '';
        if (template === 'dr-test') return !!drTargets?.[targetIndex];
        if (template === 'dr-exercise') return recordId !== '';
        return true;
    }

    /** The body of the report of the template in force, as the API takes it. */
    function requestOf(): Parameters<typeof api.generateReportV2>[0] {
        if (template === 'dr-exercise') return { type: template, record_id: recordId };
        if (template === 'dr-test') {
            const target = drTargets?.[targetIndex];
            const query = viewQuery(
                {
                    target: target?.system ?? '',
                    environment: target?.environment ?? '',
                    period: drPeriod,
                },
                Date.now(),
            );
            return {
                type: template,
                target: query.target,
                ...(query.environment ? { environment: query.environment } : {}),
                from: query.from_ns,
                to: query.to_ns,
            };
        }
        return {
            type: template,
            period,
            ...(template === 'game-day' ? { experiment_id: experimentId } : {}),
            ...(template === 'evidence-pack' ? { framework } : {}),
        };
    }

    async function loadReportsV2() {
        const r = await api.reportsV2();
        reportsV2 = r.reports;
    }

    async function generateV2() {
        if (generatingV2 || !canGenerate) return;
        generatingV2 = true;
        generateErrorV2 = null;
        try {
            const meta = await api.generateReportV2(requestOf());
            preview = meta.doc_id;
            await loadReportsV2();
        } catch (e) {
            generateErrorV2 = String(e);
        } finally {
            generatingV2 = false;
        }
    }

    // ---- v1: quick metric digests ------------------------------------------
    let reports: ReportFile[] | null = $state(null);
    let error: string | null = $state(null);

    let metrics: MetricDefInfo[] = $state([]);
    let selected = $state('');
    let generating = $state(false);
    let generateError: string | null = $state(null);
    let generated: string | null = $state(null);

    async function loadReports() {
        const r = await api.reports();
        reports = r.reports;
    }

    onMount(() => {
        api.me()
            .then((session) => (me = session))
            .catch((e) => (errorV2 = String(e)));
        loadReportsV2().catch((e) => (errorV2 = String(e)));
        loadReports().catch((e) => (error = String(e)));
        api.experiments({ range: '14d' })
            .then((r) => {
                experiments = r.experiments;
                if (experiments.length > 0) experimentId = experiments[0].id;
            })
            .catch(() => {});
        api.metrics()
            .then((m) => {
                metrics = m.metrics;
                if (metrics.length > 0) selected = metrics[0].name;
            })
            .catch(() => {});
    });

    async function generate() {
        if (!selected || generating || !canGenerate) return;
        generating = true;
        generateError = null;
        try {
            const body = await api.generateReport(selected);
            generated = body.name;
            await loadReports();
        } catch (e) {
            generateError = String(e);
        } finally {
            generating = false;
        }
    }

    function fmtSize(bytes: number): string {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
        return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    }

    function fmtNs(ns: number): string {
        return new Date(ns / 1e6).toLocaleString();
    }
</script>

<PageHeader
    title="Reports"
    sub="compliance-grade reports (PDF + print preview) and quick metric digests"
/>

<Panel title="Compliance reports" style="margin-bottom: var(--space-3-5)">
    {#if me && !canGenerate}<p>Generating reports requires the operator role or above.</p>{/if}
    <div class="genbar">
        <select bind:value={template} aria-label="template">
            {#each templates as t (t.id)}
                <option value={t.id} title={t.hint}>{t.label}</option>
            {/each}
        </select>
        {#if template === 'game-day'}
            <select
                bind:value={experimentId}
                disabled={experiments.length === 0}
                aria-label="experiment"
            >
                {#each experiments as e (e.id)}
                    <option value={e.id}
                        >{e.name ?? e.id} · {new Date(
                            e.started_ns / 1e6,
                        ).toLocaleDateString()}</option
                    >
                {/each}
            </select>
        {/if}
        {#if template === 'evidence-pack'}
            <select bind:value={framework} aria-label="framework">
                <option value="dora">DORA</option>
                <option value="nis2">NIS2</option>
                <option value="iso27001">ISO 27001</option>
                <option value="soc2">SOC 2</option>
            </select>
        {/if}
        {#if template === 'executive-digest' || template === 'evidence-pack'}
            <select bind:value={period} aria-label="period">
                <option value="24h">last 24h</option>
                <option value="7d">last 7d</option>
                <option value="14d">last 14d</option>
            </select>
        {/if}
        {#if template === 'dr-test'}
            <select bind:value={targetIndex} disabled={!drTargets?.length} aria-label="target">
                {#each drTargets ?? [] as t, i (i)}
                    <option value={i}>{targetName(t)}</option>
                {/each}
            </select>
            <select bind:value={drPeriod} aria-label="period">
                {#each Object.keys(PERIOD_DAYS) as p (p)}
                    <option value={p}>{PERIOD_LABEL[p]}</option>
                {/each}
            </select>
        {/if}
        {#if template === 'dr-exercise'}
            <select bind:value={recordId} disabled={!drRecords?.length} aria-label="record">
                {#each drRecords ?? [] as r (r.id)}
                    <option value={r.id}
                        >{r.experiment_name} · {r.exercise_type} · {localDay(
                            r.executed_at_ns,
                        )}</option
                    >
                {/each}
            </select>
        {/if}
        <button
            class="primary"
            onclick={generateV2}
            disabled={!canGenerate || generatingV2 || !hasChoices()}
        >
            {generatingV2 ? 'Rendering…' : 'Generate'}
        </button>
        {#if generateErrorV2}
            <span class="state error" style="padding: 0">{generateErrorV2}</span>
        {/if}
    </div>

    {#if errorV2}
        <div class="state error">Failed to load reports: {errorV2}</div>
    {:else if !reportsV2}
        <div class="skeleton" style="height: 120px; margin-top: var(--space-3)"></div>
    {:else if reportsV2.length === 0}
        <div class="state" style="text-align: left; margin-top: var(--space-3)">
            <b>No compliance reports yet.</b><br />
            Pick a template above and generate one — PDF and print-HTML artifacts land in
            <code>&lt;db dir&gt;/reports/v2/</code>.
        </div>
    {:else}
        <table class="data" style="margin-top: var(--space-3)">
            <thead>
                <tr
                    ><th>Document</th><th>Type</th><th>Created</th><th>Size</th><th>SHA-256</th><th
                    ></th></tr
                >
            </thead>
            <tbody>
                {#each reportsV2 as r (r.doc_id)}
                    <tr class="clickable" onclick={() => (preview = r.doc_id)}>
                        <td class="mono">{r.doc_id}</td>
                        <td><Badge tone="neutral" label={templateCode(r.type)} /></td>
                        <td>{fmtNs(r.created_ns)}</td>
                        <td class="mono">{fmtSize(r.bytes)}</td>
                        <td class="mono hash" title={r.sha256}>{r.sha256.slice(0, 10)}…</td>
                        <td>
                            <a href="/api/reports/v2/{r.doc_id}/pdf" target="_blank" rel="noopener"
                                >PDF ↗</a
                            >
                        </td>
                    </tr>
                {/each}
            </tbody>
        </table>
    {/if}

    {#if preview}
        <div class="result">
            <div class="result-head">
                <span class="mono">{preview}</span>
                <span>
                    <a href="/api/reports/v2/{preview}/pdf" target="_blank" rel="noopener"
                        >download PDF ↗</a
                    >
                    ·
                    <a href="/api/reports/v2/{preview}/html" target="_blank" rel="noopener"
                        >open in new tab ↗</a
                    >
                </span>
            </div>
            <iframe src="/api/reports/v2/{preview}/html" title="report preview" sandbox=""></iframe>
        </div>
    {/if}
</Panel>

<Panel title="Quick digests">
    <div class="genbar">
        <select bind:value={selected} disabled={metrics.length === 0} aria-label="metric">
            {#each metrics as m (m.name)}
                <option value={m.name} title={m.description ?? ''}>{m.name}</option>
            {/each}
        </select>
        <button
            class="primary"
            onclick={generate}
            disabled={!canGenerate || generating || !selected}
        >
            {generating ? 'Rendering…' : 'Generate'}
        </button>
        {#if generateError}
            <span class="state error" style="padding: 0">{generateError}</span>
        {/if}
    </div>
    {#if generated}
        <div class="result">
            <div class="result-head">
                <span class="mono">{generated}</span>
                <a href="/api/reports/{generated}" target="_blank" rel="noopener"
                    >open in new tab ↗</a
                >
            </div>
            <iframe src="/api/reports/{generated}" title="generated report" sandbox=""></iframe>
        </div>
    {/if}

    {#if error}
        <div class="state error">Failed to load reports: {error}</div>
    {:else if !reports}
        <div class="skeleton" style="height: 120px; margin-top: var(--space-3)"></div>
    {:else if reports.length > 0}
        <table class="data" style="margin-top: var(--space-3)">
            <thead>
                <tr><th>Digest</th><th>Origin</th><th>Rendered</th><th>Size</th><th></th></tr>
            </thead>
            <tbody>
                {#each reports as r (r.name)}
                    <tr class="clickable" onclick={() => (generated = r.name)}>
                        <td class="mono">{r.name}</td>
                        <td>
                            <Badge
                                kind="report"
                                value={r.name.startsWith('manual_') ? 'manual' : 'scheduled'}
                            />
                        </td>
                        <td>{new Date(r.modified_s * 1000).toLocaleString()}</td>
                        <td class="mono">{fmtSize(r.bytes)}</td>
                        <td>
                            <a href="/api/reports/{r.name}" target="_blank" rel="noopener">open ↗</a
                            >
                        </td>
                    </tr>
                {/each}
            </tbody>
        </table>
    {/if}
</Panel>

<style>
    .genbar {
        display: flex;
        gap: var(--space-2-5);
        align-items: center;
        flex-wrap: wrap;
    }
    .result {
        margin-top: var(--space-3-5);
    }
    .result-head {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
        margin-bottom: var(--space-2);
        font-size: var(--font-size-sm);
        color: var(--text-dim);
    }
    .hash {
        font-size: var(--font-size-2xs);
        color: var(--text-dim);
    }
    iframe {
        width: 100%;
        height: 560px;
        border: var(--border-width-sm) solid var(--border-strong);
        border-radius: var(--radius-lg);
        background: #fff;
    }
</style>

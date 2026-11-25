<script lang="ts">
    // The evidence of one run, as its journal in the lake says it: the
    // steady state, the objectives, the steps, the load and the samples
    // of the probes. The charts are the ones of the report `tumult report`
    // writes. A run the lake has no journal of has no evidence.
    import { onMount } from 'svelte';
    import { fmtDuration } from '#lib/api.js';
    import { chartTheme, chartTokens } from '#lib/echarts.js';
    import type { ChartTheme } from '#lib/echarts.js';
    import {
        fmtMargin,
        fmtValue,
        loadFacts,
        loadLatencyOption,
        loadThroughputOption,
        objectiveClass,
        objectiveTarget,
        phaseLabel,
        probeSamplesOption,
        recoveryRows,
        samplesNote,
        steadyState,
        stepClass,
        stepOffset,
    } from '#lib/evidence.js';
    import type { RunEvidence } from '#lib/types.js';
    import EChart from './EChart.svelte';

    let { evidence }: { evidence: RunEvidence | null } = $props();

    let theme = $state<ChartTheme>(chartTheme());
    onMount(() => {
        const retheme = () => (theme = chartTheme());
        const mo = new MutationObserver(retheme);
        mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
        const mq = window.matchMedia?.('(prefers-color-scheme: light)');
        mq?.addEventListener?.('change', retheme);
        return () => {
            mo.disconnect();
            mq?.removeEventListener?.('change', retheme);
        };
    });
    const tokens = $derived(chartTokens(theme));
</script>

{#if evidence === null}
    <div class="panel state block" data-testid="no-evidence">
        The lake has no journal of this run, so this page has no steps, objectives, load or probe
        samples to show.
    </div>
{:else}
    {@const before = evidence.hypothesis.before_met}
    {@const after = evidence.hypothesis.after_met}
    {@const recovery = recoveryRows(evidence.recovery, evidence.objectives)}
    {@const note = samplesNote(evidence)}
    <div class="grid cols-2 block">
        <div class="panel" data-testid="evidence-steady">
            <h2>Steady state</h2>
            <dl class="facts">
                <div>
                    <dt>Kind of test</dt>
                    <dd>
                        <span class="badge neutral" data-testid="run-kind">{evidence.kind}</span>
                    </dd>
                </div>
                {#if before !== null || after !== null}
                    {@const first = steadyState(before)}
                    {@const last = steadyState(after)}
                    <div>
                        <dt>Before the method</dt>
                        <dd>
                            <span class="badge {first.cls}" data-testid="steady-before"
                                >{first.label}</span
                            >
                        </dd>
                    </div>
                    <div>
                        <dt>After the method</dt>
                        <dd>
                            <span class="badge {last.cls}" data-testid="steady-after"
                                >{last.label}</span
                            >
                        </dd>
                    </div>
                {/if}
            </dl>
            {#if before === null && after === null}
                <div class="state">This run checked no steady state.</div>
            {/if}
            {#if recovery.length > 0}
                <table class="data" aria-label="Recovery">
                    <thead>
                        <tr><th>Recovery</th><th>Target</th><th>Measured</th></tr>
                    </thead>
                    <tbody>
                        {#each recovery as row (row.label)}
                            <tr>
                                <td>{row.label}</td>
                                <td class="mono">{row.target}</td>
                                <td class="mono">{row.measured}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            {/if}
        </div>

        <div class="panel" data-testid="evidence-objectives">
            <h2>Objectives ({evidence.objectives.length})</h2>
            {#if evidence.objectives.length === 0}
                <div class="state">This run had no objective.</div>
            {:else}
                <table class="data">
                    <thead>
                        <tr>
                            <th>Objective</th><th>Metric</th><th>Target</th><th>Observed</th>
                            <th>Margin</th><th>Verdict</th><th>Criticality</th>
                        </tr>
                    </thead>
                    <tbody>
                        {#each evidence.objectives as objective, i (i)}
                            <tr>
                                <td class="mono">{objective.nfr_id}</td>
                                <td class="mono">{objective.metric}</td>
                                <td class="mono">{objectiveTarget(objective)}</td>
                                <td class="mono">{fmtValue(objective.observed)}</td>
                                <td class="mono">{fmtMargin(objective.margin)}</td>
                                <td>
                                    <span class="badge {objectiveClass(objective.state)}"
                                        >{objective.state}</span
                                    >
                                </td>
                                <td>{objective.criticality}</td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            {/if}
        </div>
    </div>

    <div class="panel block" data-testid="evidence-steps">
        <h2>Steps ({evidence.steps.length})</h2>
        {#if evidence.steps.length === 0}
            <div class="state">This run took no step.</div>
        {:else}
            <table class="data">
                <thead>
                    <tr>
                        <th>Phase</th><th>Step</th><th>Type</th><th>Status</th><th>Started</th>
                        <th>Took</th><th>Result</th>
                    </tr>
                </thead>
                <tbody>
                    {#each evidence.steps as step, i (i)}
                        {@const result = step.error ?? step.output ?? ''}
                        <tr>
                            <td>{phaseLabel(step.phase)}</td>
                            <td class="mono">{step.name}</td>
                            <td>{step.activity_type}</td>
                            <td
                                ><span class="badge {stepClass(step.status)}">{step.status}</span
                                ></td
                            >
                            <td class="mono">{stepOffset(evidence, step.started_at_ns)}</td>
                            <td class="mono">{fmtDuration(step.duration_ms * 1_000_000)}</td>
                            <td class="mono result" title={result}>{result}</td>
                        </tr>
                    {/each}
                </tbody>
            </table>
        {/if}
    </div>

    <div class="panel block" data-testid="evidence-load">
        <h2>Load</h2>
        {#if evidence.load === null}
            <div class="state">This run had no load.</div>
        {:else}
            {@const latency = loadLatencyOption(evidence, tokens)}
            {@const throughput = loadThroughputOption(evidence, tokens)}
            <dl class="facts">
                {#each loadFacts(evidence.load) as [name, value] (name)}
                    <div>
                        <dt>{name}</dt>
                        <dd class="mono">{value}</dd>
                    </div>
                {/each}
            </dl>
            {#if latency !== null && throughput !== null}
                <div class="grid cols-2">
                    <EChart
                        option={latency}
                        height={240}
                        id="run-load-latency"
                        label="Latency of the load over time: p50, p95 and p99"
                    />
                    <EChart
                        option={throughput}
                        height={240}
                        id="run-load-throughput"
                        label="Throughput and error rate of the load over time"
                    />
                </div>
            {/if}
        {/if}
    </div>

    <div class="panel block" data-testid="evidence-samples">
        <h2>Probe samples ({evidence.probe_samples_total})</h2>
        {#if evidence.probe_samples.length === 0}
            <div class="state">This run took no probe samples.</div>
        {:else}
            <EChart
                option={probeSamplesOption(evidence, tokens)}
                height={260}
                id="run-probe-samples"
                label="How long each probe sample took over the time of the run"
            />
            {#if note !== null}
                <p class="note" data-testid="samples-note">{note}</p>
            {/if}
        {/if}
    </div>
{/if}

<style>
    .block {
        margin-bottom: var(--space-3-5);
    }
    .facts {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2-5) 28px;
        margin: 0 0 var(--space-2-5);
    }
    .facts dt {
        color: var(--text-dim);
        font-size: var(--font-size-3xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-md);
        margin-bottom: var(--space-1);
    }
    .facts dd {
        margin: 0;
        font-size: var(--font-size-lg);
        font-weight: var(--font-weight-strong);
    }
    .result {
        max-width: 360px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .note {
        color: var(--text-dim);
        font-size: var(--font-size-xs);
        margin: var(--space-2) 0 0;
    }
</style>

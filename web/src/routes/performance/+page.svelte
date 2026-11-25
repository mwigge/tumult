<script lang="ts">
    // Performance: the regression evidence for one target. The daemon compares
    // the candidate version's load latency with the baseline's in ONE
    // environment and answers improved / unchanged / regressed / inconclusive
    // with the interval behind it. Operators can store the verdict; the
    // server stays the authority on who may.
    import { onMount } from 'svelte';
    import { goto } from '$app/navigation';
    import { page } from '$app/state';
    import { ApiError, api, fmtAgo, fmtTs } from '#lib/api.js';
    import { chartTheme, chartTokens, followTheme } from '#lib/echarts.js';
    import type { EChartsCoreOption } from '#lib/echarts.js';
    import {
        PERF_QUANTILES,
        PERF_RANGES,
        UNTAGGED,
        attributionLabel,
        envLabel,
        fmtConfidence,
        fmtDeltaMs,
        fmtInterval,
        fmtMs,
        fmtShare,
        fmtShareDelta,
        fmtUnanswered,
        hasInferredAttribution,
        perfChartLabel,
        perfDistributionOption,
        perfRuns,
        perfSides,
        planLabel,
        redeployedVersions,
        selectionLabel,
    } from '#lib/perf.js';
    import type {
        MeResponse,
        PerfParams,
        PerfQuantile,
        PerfTargets,
        PerfVerdict,
    } from '#lib/types.js';
    import Badge from '#lib/components/Badge.svelte';
    import EChart from '#lib/components/EChart.svelte';

    // Why the read has no answer: `none` = 404 (no evidence in scope),
    // `input` = 400 (the query must be narrowed), `fail` = anything else.
    type ProblemKind = 'none' | 'input' | 'fail';
    interface Problem {
        kind: ProblemKind;
        message: string;
    }

    // The value of the disabled picker entry for telemetry without an
    // environment: it is reachable through `auto`, not through `env=`.
    const UNTAGGED_VALUE = '__untagged__';

    let me = $state<MeResponse | null>(null);
    let targets = $state<PerfTargets | null>(null);
    let listError = $state<string | null>(null);
    let answer = $state<PerfVerdict | null>(null);
    let problem = $state<Problem | null>(null);
    let loading = $state(false);
    // Versions last seen for the picked target; they outlive a 400/404 so the
    // explicit-version pickers stay usable.
    let versions = $state<string[]>([]);

    let target = $state(page.url.searchParams.get('target') ?? '');
    let env = $state(page.url.searchParams.get('env') ?? '');
    let quantile = $state<PerfQuantile>('p95');
    let range = $state('30d');
    let baseline = $state('');
    let candidate = $state('');
    let includePartial = $state(false);

    let storing = $state(false);
    let storeStatus = $state('');
    let storeError = $state<string | null>(null);

    const canStore = $derived(
        me ? !me.auth_required || (!!me.role && me.role !== 'viewer') : false,
    );
    const busy = $derived(loading || storing);

    const targetOptions = $derived.by(() => {
        const names = targets?.targets.map((t) => t.target) ?? [];
        return target && !names.includes(target) ? [target, ...names] : names;
    });
    const environments = $derived(
        targets?.targets.find((t) => t.target === target)?.environments ?? [],
    );
    const envOptions = $derived.by(() => {
        const named = environments.filter((e): e is string => e !== null);
        return env && !named.includes(env) ? [env, ...named] : named;
    });
    const hasUntagged = $derived(environments.includes(null));
    const versionOptions = $derived([
        ...new Set([...versions, baseline, candidate].filter((v) => v !== '')),
    ]);

    const sides = $derived(answer ? perfSides(answer) : []);
    const hasSamples = $derived(sides.some(({ side }) => side.distribution.length > 0));
    // The chart is drawn in the colours of the theme in force.
    let theme = $state(chartTheme());
    onMount(() => followTheme((next) => (theme = next)));
    const tokens = $derived(chartTokens(theme));
    const chartOption: EChartsCoreOption | null = $derived(
        answer ? perfDistributionOption(answer, tokens) : null,
    );

    function params(): PerfParams {
        return {
            quantile,
            env,
            baseline,
            candidate,
            range,
            ...(includePartial ? { include: 'partial' as const } : {}),
        };
    }

    function classify(e: unknown): Problem {
        const message = e instanceof Error ? e.message : String(e);
        if (!(e instanceof ApiError)) return { kind: 'fail', message };
        if (e.status === 404) return { kind: 'none', message };
        if (e.status === 400) return { kind: 'input', message };
        return { kind: 'fail', message };
    }

    // Only the newest read may land: a slower, older one is dropped.
    let readSeq = 0;

    /** Reads the answer; `keep` leaves the one on screen in place on failure. */
    async function loadAnswer(keep = false): Promise<void> {
        if (!target) return;
        const mine = ++readSeq;
        loading = true;
        if (!keep) problem = null;
        try {
            const next = await api.perf(target, params());
            if (mine !== readSeq) return;
            answer = next;
            problem = null;
            versions = next.versions_seen.map((v) => v.version);
        } catch (e) {
            if (mine !== readSeq || keep) return;
            answer = null;
            problem = classify(e);
        } finally {
            if (mine === readSeq) loading = false;
        }
    }

    async function loadTargets(): Promise<void> {
        listError = null;
        try {
            const next = await api.perfTargets(range);
            targets = next;
            if (!target) target = next.targets[0]?.target ?? '';
        } catch (e) {
            targets = null;
            listError = e instanceof Error ? e.message : String(e);
            return;
        }
        await loadAnswer();
    }

    onMount(() => {
        api.me()
            .then((m) => (me = m))
            .catch(() => (me = null));
        void loadTargets();
    });

    function syncUrl(): void {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- Local calculation; no retained reactive state.
        const q = new URLSearchParams();
        if (target) q.set('target', target);
        if (env) q.set('env', env);
        void goto(`?${q}`, { replace: true, reset: false });
    }

    function clearStoreOutcome(): void {
        storeStatus = '';
        storeError = null;
    }

    function onTarget(): void {
        env = '';
        baseline = '';
        candidate = '';
        versions = [];
        answer = null;
        clearStoreOutcome();
        syncUrl();
        void loadAnswer();
    }

    function onEnv(): void {
        clearStoreOutcome();
        syncUrl();
        void loadAnswer();
    }

    function onQuery(): void {
        clearStoreOutcome();
        void loadAnswer();
    }

    function onRange(): void {
        clearStoreOutcome();
        void loadTargets();
    }

    async function store(): Promise<void> {
        if (busy || !canStore || !answer?.baseline) return;
        storing = true;
        clearStoreOutcome();
        try {
            const stored = await api.storePerfVerdict(target, params());
            answer = stored;
            // Read the history again; the stored answer stays if that fails.
            await loadAnswer(true);
            storeStatus = stored.duplicate ? 'Already stored — nothing appended' : 'Verdict stored';
        } catch (e) {
            storeError = e instanceof Error ? e.message : String(e);
        } finally {
            storing = false;
        }
    }
</script>

<div class="page-head">
    <h1>Performance</h1>
    <span class="sub">baseline versus candidate latency per target</span>
    {#if answer}
        <span class="sub">as of {fmtAgo(answer.as_of_ns)}</span>
    {/if}
    <span class="sub" role="status" aria-live="polite">{loading ? 'Loading…' : ''}</span>
</div>

<div class="controls">
    {#if targetOptions.length > 0}
        <div class="field">
            <label for="perf-target">Target</label>
            <select
                id="perf-target"
                data-testid="target-picker"
                bind:value={target}
                onchange={onTarget}
            >
                {#each targetOptions as t (t)}
                    <option value={t}>{t}</option>
                {/each}
            </select>
        </div>
        <div class="field">
            <label for="perf-env">Environment</label>
            <select id="perf-env" bind:value={env} onchange={onEnv}>
                <option value="">auto</option>
                {#each envOptions as e (e)}
                    <option value={e}>{e}</option>
                {/each}
                {#if hasUntagged}
                    <option
                        value={UNTAGGED_VALUE}
                        disabled
                        title="telemetry that names no environment; reachable through auto"
                        >{UNTAGGED}</option
                    >
                {/if}
            </select>
        </div>
        <div class="field">
            <label for="perf-quantile">Quantile</label>
            <select id="perf-quantile" bind:value={quantile} onchange={onQuery}>
                {#each PERF_QUANTILES as q (q)}
                    <option value={q}>{q}</option>
                {/each}
            </select>
        </div>
    {/if}
    <div class="field">
        <label for="perf-range">Range</label>
        <select id="perf-range" bind:value={range} onchange={onRange}>
            {#each PERF_RANGES as r (r)}
                <option value={r}>{r}</option>
            {/each}
        </select>
    </div>
    {#if targetOptions.length > 0}
        <div class="field">
            <label for="perf-baseline">Baseline version</label>
            <select id="perf-baseline" bind:value={baseline} onchange={onQuery}>
                <option value="">deployment order</option>
                {#each versionOptions as v (v)}
                    <option value={v}>{v}</option>
                {/each}
            </select>
        </div>
        <div class="field">
            <label for="perf-candidate">Candidate version</label>
            <select id="perf-candidate" bind:value={candidate} onchange={onQuery}>
                <option value="">deployment order</option>
                {#each versionOptions as v (v)}
                    <option value={v}>{v}</option>
                {/each}
            </select>
        </div>
        <div class="field check">
            <input
                id="perf-partial"
                type="checkbox"
                bind:checked={includePartial}
                onchange={onQuery}
            />
            <label for="perf-partial">Include partial runs</label>
        </div>
    {/if}
</div>

{#if listError}
    <div class="alert" role="alert">
        <strong>Could not load the performance targets.</strong>
        <span>{listError}</span>
        <button type="button" class="btn" onclick={loadTargets}>Retry</button>
    </div>
{:else if !targets}
    <div class="state">Loading targets…</div>
{:else if targetOptions.length === 0}
    <div class="panel empty" data-testid="empty">
        <strong>No load-tested targets yet</strong>
        <p>
            Nothing in the last {range} carries load evidence. Run a load experiment with
            <code>tumult run</code> against a target whose service reports
            <code>service.version</code>.
        </p>
    </div>
{:else if problem?.kind === 'fail'}
    <div class="alert" role="alert">
        <strong>Could not load the performance evidence.</strong>
        <span>{problem.message}</span>
        <button type="button" class="btn" onclick={() => loadAnswer()}>Retry</button>
    </div>
{:else if problem?.kind === 'none'}
    <div class="panel none" data-testid="no-evidence">
        <strong>No performance evidence for <span class="mono">{target}</span></strong>
        <p>{problem.message}</p>
    </div>
{:else if problem?.kind === 'input'}
    <div class="panel needs" data-testid="needs-input">
        <strong>The comparison needs a narrower question</strong>
        <p>{problem.message}</p>
        <p class="muted">Pick an environment or explicit versions above.</p>
    </div>
{:else if !answer}
    <div class="state">Loading evidence…</div>
{:else}
    {@const cmp = answer.comparison}
    <div class="grid cols-2" aria-busy={loading}>
        <section class="panel" data-testid="verdict" aria-labelledby="perf-verdict-h">
            <h2 id="perf-verdict-h">Verdict</h2>
            <div class="headline">
                <Badge big kind="verdict" value={answer.verdict} data-testid="verdict-badge" />
                {#if cmp.low_run_count}
                    <Badge tone="warn" data-testid="few-runs" label="fewer than 5 runs" />
                {/if}
                <span class="mono">{answer.target}</span>
                <span class="muted mono">{answer.probe}</span>
            </div>
            {#if answer.reasons.length > 0}
                <ul class="reasons">
                    {#each answer.reasons as reason (reason)}
                        <li>{reason}</li>
                    {/each}
                </ul>
            {/if}
            <ul class="notes">
                <li>Selection: {selectionLabel(answer.selection)}</li>
                <li data-testid="selection-rule">{answer.selection_rule}</li>
                {#each redeployedVersions(answer) as version (version)}
                    <li data-testid="redeployed">
                        <span class="mono">{version}</span> was deployed again after a later version;
                        its latest deployment counts.
                    </li>
                {/each}
                {#if answer.backfill === 'running'}
                    <li class="caution" data-testid="backfill">
                        The version history of this lake is still being read back; the deployment
                        order is not final.
                    </li>
                {/if}
                <li>
                    Answer for environment {envLabel(answer.environment)}
                    {#if answer.environments_seen.length > 1}
                        <span class="muted">
                            (seen in {answer.environments_seen.map(envLabel).join(', ')})
                        </span>
                    {/if}
                </li>
                <li>
                    Range {answer.range}, {answer.include_partial
                        ? 'partial runs included'
                        : 'partial runs left out'}
                </li>
                {#each sides as { role, side } (role)}
                    <li data-testid="attribution-{role}">
                        Attribution, {role}
                        <span class="mono">{side.version}</span>: {attributionLabel(
                            side.attribution,
                        )}
                    </li>
                {/each}
                {#if !answer.baseline}
                    <li>Only one version is known, so there is no baseline to compare with.</li>
                {/if}
                {#if hasInferredAttribution(answer)}
                    <li class="caution">
                        Inferred attribution comes from span timing and is weaker evidence than a
                        declared or observed version.
                    </li>
                {/if}
            </ul>
        </section>

        <section class="panel" aria-labelledby="perf-delta-h">
            <h2 id="perf-delta-h">Difference at {cmp.quantile}</h2>
            <div data-testid="delta">
                {#if cmp.delta === null}
                    <div class="delta-value">No difference was computed</div>
                {:else}
                    <div class="delta-value mono">{fmtDeltaMs(cmp.delta, cmp.delta_pct)}</div>
                    <div class="muted">candidate minus baseline</div>
                {/if}
                {#if cmp.ci_low !== null && cmp.ci_high !== null}
                    <div>
                        {fmtConfidence(cmp.confidence)} confidence interval
                        <span class="mono">{fmtInterval(cmp.ci_low, cmp.ci_high)}</span> ms
                    </div>
                {/if}
                <div>noise margin {fmtMs(cmp.margin)} ms</div>
                <div class="muted">
                    iterations {cmp.iterations} · seed {cmp.seed} · minimum samples
                    {cmp.min_samples} · minimum runs {cmp.min_runs}
                </div>
                {#if cmp.low_run_count}
                    <div class="muted" data-testid="low-run-count">
                        Fewer than 5 runs on a side: the interval has few resamples to draw from, so
                        read its bounds as coarse.
                    </div>
                {/if}
            </div>
        </section>
    </div>

    <section class="panel below" aria-labelledby="perf-dist-h" aria-busy={loading}>
        <h2 id="perf-dist-h">Latency distribution</h2>
        {#if chartOption && hasSamples}
            <div role="img" aria-label={perfChartLabel(answer)}>
                <EChart option={chartOption} id="perf" height={280} />
            </div>
        {:else}
            <div class="state">No samples to plot.</div>
        {/if}
        <table class="data" aria-label="Latency by version">
            <thead>
                <tr>
                    <th scope="col">Side</th>
                    <th scope="col">Version</th>
                    <th scope="col">Runs</th>
                    <th scope="col">Samples</th>
                    <th scope="col">Dropped</th>
                    <th scope="col">p50 ms</th>
                    <th scope="col">p95 ms</th>
                    <th scope="col">p99 ms</th>
                </tr>
            </thead>
            <tbody>
                {#each sides as { role, side } (role)}
                    <tr>
                        <th scope="row">{role}</th>
                        <td class="mono">{side.version}</td>
                        <td class="mono">{side.runs.length}</td>
                        <td class="mono">{side.samples}</td>
                        <td class="mono">{side.dropped}</td>
                        <td class="mono">{fmtMs(side.p50)}</td>
                        <td class="mono">{fmtMs(side.p95)}</td>
                        <td class="mono">{fmtMs(side.p99)}</td>
                    </tr>
                {/each}
            </tbody>
        </table>
    </section>

    <section class="panel below" aria-labelledby="perf-partial-h">
        <h2 id="perf-partial-h">Partial runs and windows</h2>
        <table class="data" aria-label="Partial runs and windows by version">
            <thead>
                <tr>
                    <th scope="col">Side</th>
                    <th scope="col">Version</th>
                    <th scope="col">Truncated</th>
                    <th scope="col">Aborted</th>
                    <th scope="col">Halted</th>
                    <th scope="col">Deviated</th>
                    <th scope="col">Excluded</th>
                    <th scope="col">Windows planned</th>
                    <th scope="col">Completed</th>
                    <th scope="col">Unanswered</th>
                    <th scope="col">Lost</th>
                    <th scope="col">Censored</th>
                </tr>
            </thead>
            <tbody>
                {#each sides as { role, side } (role)}
                    <tr>
                        <th scope="row">{role}</th>
                        <td class="mono">{side.version}</td>
                        <td class="mono">{side.partial.truncated}</td>
                        <td class="mono">{side.partial.aborted}</td>
                        <td class="mono">{side.partial.halted}</td>
                        <td class="mono">{side.partial.deviated}</td>
                        <td class="mono">{side.partial.excluded}</td>
                        <td class="mono">{side.windows.planned}</td>
                        <td class="mono">{side.windows.completed}</td>
                        <td class="mono">{fmtUnanswered(side.windows)}</td>
                        <td class="mono">{side.windows.lost}</td>
                        <td class="mono">{side.windows.censored}</td>
                    </tr>
                {/each}
            </tbody>
        </table>
        <p class="muted">
            Counts are runs per side; excluded runs are left out of the samples. Unanswered is the
            share of the windows the load reached; lost windows were skipped behind an unanswered
            one. A censored window reports a request given up on at its timeout: the true value is
            that or more.
        </p>
        <table class="data" aria-label="Requests by version">
            <thead>
                <tr>
                    <th scope="col">Side</th>
                    <th scope="col">Version</th>
                    <th scope="col">Requests sent</th>
                    <th scope="col">Failed</th>
                    <th scope="col">Failure share</th>
                    <th scope="col">Runs without counts</th>
                </tr>
            </thead>
            <tbody>
                {#each sides as { role, side } (role)}
                    <tr>
                        <th scope="row">{role}</th>
                        <td class="mono">{side.version}</td>
                        <td class="mono">{side.requests.sent}</td>
                        <td class="mono">{side.requests.failed}</td>
                        <td class="mono">{fmtShare(side.requests.failure_share)}</td>
                        <td class="mono">{side.requests.runs_without_counts}</td>
                    </tr>
                {/each}
            </tbody>
        </table>
        {#if cmp.failure_share}
            <p data-testid="failure-share">
                Failure share, candidate minus baseline: {fmtShareDelta(
                    cmp.failure_share.delta,
                    cmp.failure_share.margin,
                )}.
            </p>
        {/if}
        <p class="muted">
            Failed requests were sent and not answered well: timed out, refused or answered with an
            error. The requests of every run are counted, those of the partial runs too.
        </p>
        <table class="data" aria-label="Runs by version">
            <thead>
                <tr>
                    <th scope="col">Side</th>
                    <th scope="col">Run</th>
                    <th scope="col">Version known</th>
                    <th scope="col">Status</th>
                    <th scope="col">Plan</th>
                    <th scope="col">In the samples</th>
                </tr>
            </thead>
            <tbody>
                {#each perfRuns(answer) as { role, run } (`${role} ${run.experiment_id}`)}
                    <tr>
                        <th scope="row">{role}</th>
                        <td class="mono">{run.experiment_id}</td>
                        <td class="mono">{run.attribution}</td>
                        <td class="mono">{run.status ?? '–'}</td>
                        <td class="mono">{planLabel(run)}</td>
                        <td class="mono">{run.included ? 'yes' : 'left out'}</td>
                    </tr>
                {/each}
            </tbody>
        </table>
        {#if answer.unattributed_runs.count > 0 || answer.runs_on_other_versions > 0}
            <div class="unattributed" data-testid="unattributed">
                {#if answer.unattributed_runs.count > 0}
                    <p>
                        {answer.unattributed_runs.count}
                        {answer.unattributed_runs.count === 1 ? 'run' : 'runs'} could not be attributed
                        to a version:
                    </p>
                    <ul>
                        {#each Object.entries(answer.unattributed_runs.reasons) as [reason, count] (reason)}
                            <li class="mono">{reason}: {count}</li>
                        {/each}
                    </ul>
                {/if}
                {#if answer.runs_on_other_versions > 0}
                    <p>
                        {answer.runs_on_other_versions}
                        {answer.runs_on_other_versions === 1 ? 'run' : 'runs'} on other versions, not
                        part of this comparison.
                    </p>
                {/if}
            </div>
        {/if}
    </section>

    <section class="panel below" aria-labelledby="perf-history-h">
        <h2 id="perf-history-h">Stored verdicts</h2>
        <div class="store">
            <button
                type="button"
                class="primary"
                data-testid="store-button"
                disabled={!canStore || !answer.baseline}
                aria-disabled={busy}
                onclick={store}
            >
                Store verdict
            </button>
            {#if me && !canStore}
                <span class="muted">Storing a verdict needs the operator role.</span>
            {:else if !answer.baseline}
                <span class="muted">Storing needs two versions to compare.</span>
            {/if}
            <span class="ok-line" data-testid="store-status" role="status" aria-live="polite"
                >{storeStatus}</span
            >
            {#if storeError}
                <span class="err-line" data-testid="store-error" role="alert">
                    Not stored: {storeError}
                </span>
            {/if}
        </div>
        {#if answer.history.length === 0}
            <div class="state">No verdict has been stored for this target yet.</div>
        {:else}
            <table class="data" aria-label="Stored verdicts">
                <thead>
                    <tr>
                        <th scope="col">Computed</th>
                        <th scope="col">Baseline → candidate</th>
                        <th scope="col">Quantile</th>
                        <th scope="col">Verdict</th>
                        <th scope="col">Delta</th>
                        <th scope="col">Interval ms</th>
                        <th scope="col">Environments</th>
                    </tr>
                </thead>
                <tbody>
                    {#each answer.history as row, i (i)}
                        <tr>
                            <td class="mono">{fmtTs(row.computed_at_ns)}</td>
                            <td class="mono">{row.baseline_version} → {row.candidate_version}</td>
                            <td class="mono">{row.quantile}</td>
                            <td><Badge kind="verdict" value={row.verdict} /></td>
                            <td class="mono">{fmtDeltaMs(row.delta, row.delta_pct)}</td>
                            <td class="mono">{fmtInterval(row.ci_low, row.ci_high)}</td>
                            <td class="mono">{row.envs?.join(', ') || '–'}</td>
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
    .controls {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-3) var(--space-4);
        align-items: flex-end;
        margin-bottom: var(--space-3-5);
    }
    .field {
        display: flex;
        flex-direction: column;
        gap: var(--space-1);
    }
    .field label {
        color: var(--text-dim);
        font-size: var(--font-size-2xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-sm);
    }
    .field.check {
        flex-direction: row;
        align-items: center;
        gap: var(--space-1-5);
        padding-bottom: var(--space-1-5);
    }
    .field.check label {
        text-transform: none;
        letter-spacing: 0;
        font-size: var(--font-size-md);
        color: var(--text);
    }
    select:focus-visible,
    input:focus-visible,
    button:focus-visible {
        outline: var(--border-width-md) solid var(--accent);
        outline-offset: 2px;
    }
    .below {
        margin-top: var(--space-3-5);
    }
    .headline {
        display: flex;
        align-items: baseline;
        gap: var(--space-3);
        margin-bottom: var(--space-2-5);
        flex-wrap: wrap;
    }
    .reasons {
        margin: 0 0 var(--space-2-5);
        padding-left: var(--space-4-5);
    }
    .notes {
        list-style: none;
        margin: 0;
        padding: 0;
        color: var(--text-dim);
        font-size: var(--font-size-sm);
    }
    .notes li {
        padding: 3px 0;
        border-top: var(--border-width-sm) solid var(--border);
    }
    .notes .caution {
        color: var(--warn);
    }
    .delta-value {
        font-size: var(--font-size-3xl);
        font-weight: var(--font-weight-strong);
    }
    table.data th[scope='row'] {
        text-align: left;
        font-weight: var(--font-weight-strong);
        padding: 7px var(--space-2-5);
        border-bottom: var(--border-width-sm) solid var(--border);
        text-transform: none;
        letter-spacing: 0;
        font-size: var(--font-size-md);
        color: var(--text);
    }
    table.data tr:last-child th[scope='row'] {
        border-bottom: none;
    }
    .unattributed {
        margin-top: var(--space-2-5);
        padding-top: var(--space-2);
        border-top: var(--border-width-sm) solid var(--border);
        font-size: var(--font-size-md);
    }
    .unattributed p {
        margin: 0 0 var(--space-1);
    }
    .unattributed ul {
        margin: 0 0 var(--space-1-5);
        padding-left: var(--space-4-5);
    }
    .store {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2-5);
        align-items: center;
        margin-bottom: var(--space-3);
    }
    .store button[aria-disabled='true'] {
        opacity: 0.6;
        cursor: progress;
    }
    .ok-line {
        color: var(--ok);
        font-size: var(--font-size-sm);
    }
    .err-line {
        color: var(--fail);
        font-size: var(--font-size-sm);
    }
    /* The four non-result states differ in frame, hue and wording. */
    .alert {
        display: flex;
        flex-wrap: wrap;
        gap: var(--space-2-5);
        align-items: center;
        padding: var(--space-3-5) var(--space-4);
        border: var(--border-width-sm) solid var(--fail);
        border-left-width: var(--border-width-lg);
        border-radius: var(--radius-xl);
        background: color-mix(in srgb, var(--fail) 10%, transparent);
        color: var(--text);
    }
    .alert strong {
        color: var(--fail);
    }
    .empty,
    .none,
    .needs {
        padding: var(--space-5-5);
    }
    .empty {
        border-style: dashed;
        text-align: center;
    }
    .none {
        border-left: var(--border-width-lg) solid var(--border-strong);
    }
    .needs {
        border-left: var(--border-width-lg) solid var(--warn);
    }
    .empty p,
    .none p,
    .needs p {
        margin: var(--space-1-5) 0 0;
        color: var(--text-dim);
        font-size: var(--font-size-md);
    }
</style>

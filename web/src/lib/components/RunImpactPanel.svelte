<script lang="ts">
    // Impact panel for a finished run: each probe's observed series against
    // its counterfactual ("what the metric would have done without the
    // fault") with the 10–90 % band, the fault window shaded, and the peak
    // and recovery moments marked. The provenance line names the model that
    // produced the counterfactual. A run without probe samples (404) shows
    // nothing but a one-line note; any other failure is an error and says so.
    import { ApiError, api } from '#lib/api.js';
    import type { ProbeImpact, RunImpact } from '#lib/types.js';
    import FanChart from './FanChart.svelte';

    let { runId }: { runId: string } = $props();

    let impact = $state<RunImpact | null>(null);
    // 404: the run has nothing to compare. Not an error.
    let empty = $state(false);
    let failure = $state<string | null>(null);

    function failed(e: unknown): void {
        if (e instanceof ApiError && e.status === 404) {
            empty = true;
            return;
        }
        failure = e instanceof Error ? e.message : String(e);
    }

    $effect(() => {
        const id = runId;
        let cancelled = false;
        impact = null;
        empty = false;
        failure = null;
        api.runImpact(id)
            .then((r) => {
                if (!cancelled) impact = r;
            })
            .catch((e: unknown) => {
                if (!cancelled) failed(e);
            });
        return () => {
            cancelled = true;
        };
    });

    // The daemon reports why it fell back as a code; unknown codes are
    // shown as they are.
    const FALLBACKS: Record<string, string> = {
        unreachable: 'sidecar unreachable',
        timeout: 'sidecar timed out',
        bad_response: 'sidecar answer not usable',
        rejected: 'sidecar refused the request',
        circuit_open: 'sidecar circuit open',
    };

    function fallback(code: string): string {
        return FALLBACKS[code] ?? code;
    }

    const DEGENERATE =
        'flat reference: the band had no width and was floored, read the multiples with care';

    function num(v: number | null | undefined, digits = 1): string {
        return v == null ? '—' : v.toFixed(digits);
    }

    // A multiple of the band half-width; no unit on a value that is missing.
    function multiple(v: number | null | undefined): string {
        return v == null ? '—' : `${num(v)}×`;
    }

    function seconds(v: number | null | undefined): string {
        return v == null ? 'not recovered' : `${num(v, v < 10 ? 1 : 0)} s`;
    }

    // The headline is "not recovered" as soon as one probe never settled;
    // this says how many of the evaluated probes that was.
    function unrecovered(r: RunImpact): string {
        const n = r.unrecovered_probes ?? 0;
        const of = r.evaluated_probes ?? n;
        return `${n} of ${of} ${of === 1 ? 'probe' : 'probes'} did not settle back inside the band`;
    }

    function pct(v: number | null | undefined): string {
        return v == null ? '—' : `${Math.round(v * 100)} %`;
    }

    function marks(p: ProbeImpact): { at: number; label: string }[] {
        const out: { at: number; label: string }[] = [];
        if (p.impact?.during_peak_ts_ms != null) {
            out.push({ at: p.impact.during_peak_ts_ms, label: 'peak' });
        }
        if (p.recovery?.recovered_at_ms != null) {
            out.push({ at: p.recovery.recovered_at_ms, label: 'recovered' });
        }
        return out;
    }

    function referenceLabel(p: ProbeImpact): string {
        return p.reference === 'post_backcast'
            ? 'backcast from the post-fault samples'
            : 'forecast from the pre-fault samples';
    }
</script>

<div class="panel" style="margin-bottom: 14px" data-testid="impact">
    <h2>Impact</h2>
    {#if failure}
        <div class="state error" role="alert" data-testid="impact-error">
            The impact could not be loaded: {failure}
        </div>
    {:else if empty}
        <div class="state">No probe samples to compare for this run.</div>
    {:else if !impact}
        <div
            class="skeleton"
            style="height: 200px"
            role="status"
            aria-busy="true"
            aria-label="Loading the impact"
        ></div>
    {:else}
        <div class="summary" data-testid="impact-summary">
            {#if impact.impact_peak == null}
                <span>peak impact <strong>not measurable</strong></span>
            {:else}
                <span>
                    peak impact <strong>{num(impact.impact_peak)}×</strong> band half-width
                </span>
            {/if}
            <span>recovery <strong>{seconds(impact.recovery_time_s)}</strong></span>
            {#if impact.unrecovered_probes}
                <span class="warn" data-testid="impact-unrecovered">
                    {unrecovered(impact)}
                </span>
            {/if}
            {#if impact.degenerate_band}
                <span class="warn" data-testid="impact-degenerate">{DEGENERATE}</span>
            {/if}
            {#if impact.provenance}
                <span class="prov" data-testid="impact-provenance">
                    model {impact.provenance.model}
                    {impact.provenance.version} · weights {impact.provenance.weights_hash}
                    {#if impact.provenance.fallback_reason}
                        <span data-testid="impact-fallback">
                            · fallback: {fallback(impact.provenance.fallback_reason)}
                        </span>
                    {/if}
                </span>
            {/if}
        </div>
        {#each impact.probes as p, i (p.probe)}
            <div class="probe">
                <h3>{p.probe}</h3>
                {#if p.error || !p.series || !p.fault}
                    <div class="state">{p.error ?? 'no comparable samples'}</div>
                {:else}
                    <FanChart
                        id="impact-{i}"
                        series={p.series}
                        label="{p.probe}: observed values against the counterfactual forecast"
                        windows={[{ from: p.fault.start_ms, to: p.fault.end_ms, label: 'fault' }]}
                        marks={marks(p)}
                    />
                    <div class="facts">
                        <span>peak {multiple(p.impact?.during_peak)}</span>
                        <span>mean during fault {multiple(p.impact?.during_mean)}</span>
                        <span>outside the band {pct(p.impact?.during_outside_ratio)}</span>
                        <span>recovery {seconds(p.recovery?.recovery_time_s)}</span>
                        <span>counterfactual: {referenceLabel(p)}</span>
                        {#if p.degenerate_band}
                            <span class="warn" data-testid="impact-degenerate">{DEGENERATE}</span>
                        {/if}
                        {#if p.forecast?.insufficient_history}
                            <span class="warn">
                                short history ({p.forecast.context_points} points): flat band, read with
                                care
                            </span>
                        {/if}
                    </div>
                {/if}
            </div>
        {/each}
    {/if}
</div>

<style>
    .summary,
    .facts {
        display: flex;
        flex-wrap: wrap;
        gap: 6px 18px;
        font-size: 12.5px;
        color: var(--text-dim);
    }
    .summary {
        margin-bottom: 10px;
    }
    .summary strong {
        color: var(--text);
    }
    .prov {
        font-family: var(--mono, monospace);
    }
    .probe + .probe {
        margin-top: 16px;
    }
    .probe h3 {
        font-size: 13px;
        margin: 0 0 4px;
    }
    .warn {
        color: var(--warn);
    }
</style>

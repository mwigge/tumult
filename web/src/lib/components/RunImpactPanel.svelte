<script lang="ts">
    // Impact panel for a finished run: each probe's observed series against
    // its counterfactual ("what the metric would have done without the
    // fault") with the 10–90 % band, the fault window shaded, and the peak
    // and recovery moments marked. The provenance line names the model that
    // produced the counterfactual. A run without probe samples (404) shows
    // nothing but a one-line note.
    import { api } from '#lib/api.js';
    import type { ProbeImpact, RunImpact } from '#lib/types.js';
    import FanChart from './FanChart.svelte';

    let { runId }: { runId: string } = $props();

    let impact = $state<RunImpact | null>(null);
    let missing = $state<string | null>(null);

    $effect(() => {
        const id = runId;
        let cancelled = false;
        impact = null;
        missing = null;
        api.runImpact(id)
            .then((r) => {
                if (!cancelled) impact = r;
            })
            .catch((e: unknown) => {
                if (!cancelled) missing = String(e);
            });
        return () => {
            cancelled = true;
        };
    });

    function num(v: number | null | undefined, digits = 1): string {
        return v == null ? '—' : v.toFixed(digits);
    }

    function seconds(v: number | null | undefined): string {
        return v == null ? 'not recovered' : `${num(v, v < 10 ? 1 : 0)} s`;
    }

    function pct(v: number | null | undefined): string {
        return v == null ? '—' : `${Math.round(v * 100)} %`;
    }

    function marks(p: ProbeImpact): { at: number; label: string }[] {
        const out: { at: number; label: string }[] = [];
        if (p.impact?.during_peak_ts_ms != null) {
            out.push({ at: p.impact.during_peak_ts_ms, label: 'peak' });
        }
        if (p.recovery?.recovered_at_ns != null) {
            out.push({ at: p.recovery.recovered_at_ns / 1e6, label: 'recovered' });
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
    {#if missing}
        <div class="state">No probe samples to compare for this run.</div>
    {:else if !impact}
        <div class="skeleton" style="height: 200px"></div>
    {:else}
        <div class="summary" data-testid="impact-summary">
            <span>peak impact <strong>{num(impact.impact_peak)}×</strong> band half-width</span>
            <span>recovery <strong>{seconds(impact.recovery_time_s)}</strong></span>
            {#if impact.provenance}
                <span class="prov" data-testid="impact-provenance">
                    model {impact.provenance.model}
                    {impact.provenance.version} · weights {impact.provenance.weights_hash}
                    {#if impact.provenance.fallback_reason}
                        · fallback: {impact.provenance.fallback_reason}
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
                        windows={[
                            {
                                from: p.fault.start_ns / 1e6,
                                to: p.fault.end_ns / 1e6,
                                label: 'fault',
                            },
                        ]}
                        marks={marks(p)}
                    />
                    <div class="facts">
                        <span>peak {num(p.impact?.during_peak)}×</span>
                        <span>mean during fault {num(p.impact?.during_mean)}×</span>
                        <span>outside the band {pct(p.impact?.during_outside_ratio)}</span>
                        <span>recovery {seconds(p.recovery?.recovery_time_s)}</span>
                        <span>counterfactual: {referenceLabel(p)}</span>
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

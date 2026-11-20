<script lang="ts">
    import { page } from '$app/state';
    import { onMount } from 'svelte';
    import { ACTIVE_RUN_STATES, ApiError, api } from '#lib/api.js';
    import type { GameDayDetail, MeResponse, RunRow } from '#lib/types.js';
    import StatusBadge from '#lib/components/StatusBadge.svelte';

    const id = $derived(page.params.id ?? '');

    let me = $state<MeResponse | null>(null);
    let day = $state<GameDayDetail | null>(null);
    let error = $state<string | null>(null);

    // The newest campaign of this gameday and its child runs.
    let campaign = $state<RunRow | null>(null);
    let children = $state<RunRow[]>([]);
    let launchError = $state<string | null>(null);
    let launching = $state(false);
    // The environment the campaign runs in, typed as on New run.
    let environment = $state('');
    // Polls the campaign at once; the effect below sets it while the page is open.
    let follow: (() => void) | null = null;

    const canRun = $derived(
        me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false,
    );
    const campaignActive = $derived(campaign !== null && ACTIVE_RUN_STATES.has(campaign.state));
    const passedCount = $derived(children.filter((c) => c.state === 'passed').length);

    onMount(() => {
        api.me()
            .then((m) => (me = m))
            .catch(() => (me = null));
    });

    // Load the plan once; poll the newest campaign every 3s while it is
    // active (and for one grace poll after the last child flips terminal).
    $effect(() => {
        const gamedayId = id;
        let cancelled = false;
        let timer: ReturnType<typeof setTimeout> | null = null;
        day = null;
        campaign = null;
        children = [];

        async function load() {
            try {
                day = await api.gameday(gamedayId);
            } catch (e) {
                if (!cancelled) error = String(e);
            }
        }

        async function poll() {
            try {
                const r = await api.gamedayCampaigns(gamedayId);
                if (cancelled) return;
                const mine = r.runs
                    .filter((run) => !run.gameday_id)
                    .sort((a, b) => b.queued_at_ns - a.queued_at_ns);
                campaign = mine[0] ?? null;
                if (campaign) {
                    const kids = await api.campaignRuns(campaign.id);
                    if (cancelled) return;
                    // The runs list is newest-first; steps map to children in fire order.
                    children = kids.runs.sort((a, b) => a.queued_at_ns - b.queued_at_ns);
                }
            } catch {
                // Campaign progress is best-effort; the plan above stays.
            }
            if (!cancelled && campaign && ACTIVE_RUN_STATES.has(campaign.state)) {
                // One pending tick at most, also when a launch started a poll of its own.
                if (timer) clearTimeout(timer);
                timer = setTimeout(poll, 3000);
            }
        }

        follow = () => void poll();
        void load();
        void poll();
        return () => {
            cancelled = true;
            follow = null;
            if (timer) clearTimeout(timer);
        };
    });

    async function launch() {
        const env = environment.trim();
        if (launching || !env) return;
        launching = true;
        launchError = null;
        try {
            const started = await api.startCampaign(id, env);
            campaign = (await api.run(started.run_id)).run;
            children = [];
            follow?.();
        } catch (e) {
            launchError =
                e instanceof ApiError && e.status === 403
                    ? `Not allowed to run in ${env}: ${e.message}`
                    : String(e);
        } finally {
            launching = false;
        }
    }
</script>

{#if error && !day}
    <div class="page-head">
        <a href="/gamedays" style="color: var(--text-dim)">← gamedays</a>
        <h1>GameDay</h1>
    </div>
    <div class="state error panel">{error}</div>
{:else if !day}
    <div class="page-head">
        <a href="/gamedays" style="color: var(--text-dim)">← gamedays</a>
        <h1>GameDay</h1>
    </div>
    <div class="skeleton" style="height: 200px"></div>
{:else}
    <div class="page-head">
        <a href="/gamedays" style="color: var(--text-dim)">← gamedays</a>
        <h1>{day.title}</h1>
        <span class="sub">
            {day.experiments.length} experiments · pass threshold {day.scoring.pass_threshold} · MTTR
            target {day.scoring.mttr_target_s}s
        </span>
    </div>

    {#if day.description}
        <p class="desc">{day.description}</p>
    {/if}
    {#if day.tags.length > 0}
        <p class="tags">
            {#each day.tags as tag (tag)}<span class="badge neutral">{tag}</span>{/each}
        </p>
    {/if}

    <div class="panel" style="margin-bottom: 14px">
        <h2>Campaign steps</h2>
        <table class="data">
            <thead>
                <tr
                    ><th>#</th><th>Experiment</th><th>Compliance maps</th>{#if campaign}<th>Run</th
                        >{/if}</tr
                >
            </thead>
            <tbody>
                {#each day.experiments as step, i (step.path)}
                    {@const child = children[i]}
                    <tr>
                        <td class="mono" style="color: var(--text-faint)">{i + 1}</td>
                        <td>{step.name ?? step.path}</td>
                        <td>
                            {#if step.compliance_maps.length > 0}
                                {step.compliance_maps.join(', ')}
                            {:else}
                                <span style="color: var(--text-faint)">—</span>
                            {/if}
                        </td>
                        {#if campaign}
                            <td>
                                {#if child}
                                    <a href="/runs/{child.id}"
                                        ><StatusBadge status={child.state} /></a
                                    >
                                {:else}
                                    <span style="color: var(--text-faint)">waiting</span>
                                {/if}
                            </td>
                        {/if}
                    </tr>
                {/each}
            </tbody>
        </table>
    </div>

    {#if day.regulatory}
        <div class="panel" style="margin-bottom: 14px">
            <h2>Regulatory mapping — {day.regulatory.frameworks.join(', ')}</h2>
            <table class="data">
                <thead>
                    <tr><th>Requirement</th><th>Description</th><th>Evidence</th></tr>
                </thead>
                <tbody>
                    {#each day.regulatory.requirements as req (req.id)}
                        <tr><td>{req.id}</td><td>{req.description}</td><td>{req.evidence}</td></tr>
                    {/each}
                </tbody>
            </table>
        </div>
    {/if}

    <div class="panel">
        <h2>Run GameDay</h2>
        {#if campaign}
            <div class="campaign">
                Campaign <a href="/runs/{campaign.id}" class="mono">{campaign.id.slice(0, 8)}</a>
                <StatusBadge status={campaign.state} />
                · {passedCount}/{children.length || day.experiments.length} steps passed
            </div>
        {/if}
        {#if !canRun}
            <div class="hint">Launching campaigns requires the operator role or above.</div>
        {:else}
            <div class="params">
                <label
                    >Environment<input
                        type="text"
                        bind:value={environment}
                        placeholder="staging or production"
                    /></label
                >
            </div>
            <p class="hint">
                The administrator must bind the reviewed execution fingerprint of each step to this
                environment. Unbound executions require the highest approval tier.
            </p>
            <button
                class="primary"
                onclick={launch}
                disabled={launching || campaignActive || !environment.trim()}
            >
                {launching ? 'Starting…' : campaignActive ? 'Campaign running' : 'Run GameDay'}
            </button>
            <div class="hint">
                {environment.trim()
                    ? `The campaign runs in ${environment.trim()}.`
                    : 'Enter the environment the campaign runs in.'}
            </div>
            {#if launchError}<div class="state error" style="margin-top: 10px">
                    {launchError}
                </div>{/if}
        {/if}
    </div>
{/if}

<style>
    .desc {
        color: var(--text-dim);
        font-size: 13px;
        margin: 0 0 10px;
    }
    .tags {
        display: flex;
        gap: 6px;
        margin: 0 0 14px;
    }
    .campaign {
        margin-bottom: 10px;
        font-size: 13px;
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .params {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
        gap: 10px 18px;
    }
    .params input {
        width: 100%;
    }
    .hint {
        color: var(--text-dim);
        font-size: 12.5px;
        margin-top: 8px;
    }
</style>

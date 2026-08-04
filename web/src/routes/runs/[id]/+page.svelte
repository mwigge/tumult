<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { ACTIVE_RUN_STATES, api } from '$lib/api';
  import type { DryRunScope, ExperimentDetail, MeResponse, RunDetail, Span } from '$lib/types';
  import Waterfall from '$lib/components/Waterfall.svelte';
  import SpanDrawer from '$lib/components/SpanDrawer.svelte';
  import RunHeader from '$lib/components/RunHeader.svelte';
  import ApprovalActions from '$lib/components/ApprovalActions.svelte';
  import AuditTimeline from '$lib/components/AuditTimeline.svelte';
  import ScopeSummary from '$lib/components/ScopeSummary.svelte';

  const id = $derived($page.params.id ?? '');

  let detail = $state<RunDetail | null>(null);
  let error = $state<string | null>(null);
  let me = $state<MeResponse | null>(null);
  let telemetry = $state<ExperimentDetail | null>(null);
  let selected = $state<Span | null>(null);
  // Blast-radius summary from the run's own definition+vars, fetched once
  // via the dry-run endpoint (Viewer-level; executes nothing).
  let scope = $state<DryRunScope | null>(null);
  let scopeFailed = $state(false);

  // Approvers and admins may decide; only admins may break glass. The server
  // enforces both regardless — these only hide what the role cannot use.
  const canDecide = $derived(
    me
      ? !me.auth_required ||
          (me.authenticated && (me.role === 'approver' || me.role === 'admin'))
      : false
  );
  const isAdmin = $derived(
    me ? !me.auth_required || (me.authenticated && me.role === 'admin') : false
  );

  // Set by /runs/new when a gated start redirected here: shows the
  // "awaiting approval" notice while the run is still pending.
  const awaitingTier = $derived($page.url.searchParams.get('awaiting'));

  // Operators and up may stop runs; missing role counts as viewer. When the
  // daemon has no users (open local mode) anyone may.
  const canStop = $derived(
    me ? !me.auth_required || (me.authenticated && !!me.role && me.role !== 'viewer') : false
  );
  const active = $derived(detail !== null && ACTIVE_RUN_STATES.has(detail.run.state));

  // The actor of the triggering (enqueued) audit event, else the first
  // audit entry that carries an actor.
  const triggerActor = $derived.by(() => {
    if (!detail) return null;
    return (
      detail.audit.find((a) => a.event === 'enqueued')?.actor ??
      detail.audit.find((a) => a.actor)?.actor ??
      null
    );
  });

  onMount(() => {
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
  });

  // Poll the run (and, once linked, its experiment telemetry) every 2s while
  // the state is active. Telemetry lags the run record — the batch span
  // exporter's final flush lands after the state flips — so a terminal run
  // whose spans have not arrived yet keeps polling for a short grace window
  // before the loop stops. The cleanup clears the pending tick, so no
  // intervals leak.
  $effect(() => {
    const runId = id;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let lateTicks = 0;
    detail = null;
    telemetry = null;
    error = null;
    scope = null;
    scopeFailed = false;

    async function poll() {
      try {
        const d = await api.run(runId);
        if (cancelled) return;
        detail = d;
        error = null;
        if (!scope && !scopeFailed) {
          try {
            const vars = d.run.params_json
              ? (JSON.parse(d.run.params_json) as Record<string, string>)
              : {};
            const r = await api.dryRun(d.run.registry_id, vars);
            if (cancelled) return;
            if (r.valid) scope = r.plan.scope;
            else scopeFailed = true;
          } catch {
            if (!cancelled) scopeFailed = true;
          }
        }
        if (d.run.experiment_id) {
          try {
            const t = await api.experiment(d.run.experiment_id);
            if (!cancelled) telemetry = t;
          } catch {
            // Telemetry lags the run record; keep the previous snapshot.
          }
        }
        if (cancelled) return;
        if (ACTIVE_RUN_STATES.has(d.run.state)) {
          timer = setTimeout(poll, 2000);
        } else if (
          d.run.experiment_id &&
          (!telemetry || telemetry.spans.length === 0) &&
          lateTicks < 10
        ) {
          // Terminal but spans not ingested yet: bounded grace polling.
          lateTicks += 1;
          timer = setTimeout(poll, 2000);
        }
      } catch (e) {
        if (cancelled) return;
        if (!detail) {
          error = String(e);
        } else if (ACTIVE_RUN_STATES.has(detail.run.state)) {
          // Transient failure mid-run: keep the last snapshot, retry.
          timer = setTimeout(poll, 2000);
        }
      }
    }

    void poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  });
</script>

{#if error && !detail}
  <div class="page-head">
    <a href="/runs" style="color: var(--text-dim)">← runs</a>
    <h1>Run</h1>
  </div>
  <div class="state error panel">{error}</div>
{:else if !detail}
  <div class="page-head">
    <a href="/runs" style="color: var(--text-dim)">← runs</a>
    <h1>Run</h1>
  </div>
  <div class="skeleton" style="height: 120px; margin-bottom: 14px"></div>
  <div class="skeleton" style="height: 320px"></div>
{:else}
  {@const run = detail.run}
  <RunHeader {run} runId={id} {active} {canStop} {awaitingTier} {triggerActor} />

  {#if scope}
    <div class="panel" style="margin-bottom: 14px">
      <h2>Blast radius</h2>
      <ScopeSummary {scope} />
    </div>
  {/if}

  <div class="panel" style="margin-bottom: 14px">
    <h2>Telemetry</h2>
    {#if !run.experiment_id}
      <div class="state">Telemetry appears once the run starts executing.</div>
    {:else if !telemetry}
      <div class="skeleton" style="height: 200px"></div>
    {:else if telemetry.spans.length === 0}
      <div class="state">No spans recorded yet.</div>
    {:else}
      <Waterfall spans={telemetry.spans} onselect={(s) => (selected = s)} />
      <div class="exp-link">
        <a href="/experiments/{run.experiment_id}">open the full experiment view →</a>
      </div>
    {/if}
  </div>

  <ApprovalActions runId={id} runState={run.state} approval={detail.approval} {canDecide} {isAdmin} />

  <AuditTimeline audit={detail.audit} />

  {#if selected && telemetry}
    <SpanDrawer span={selected} logs={telemetry.logs} onclose={() => (selected = null)} />
  {/if}
{/if}

<style>
  .exp-link {
    margin-top: 10px;
    font-size: 12.5px;
  }
</style>

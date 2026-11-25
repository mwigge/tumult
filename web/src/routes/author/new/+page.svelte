<script lang="ts">
    import { onMount } from 'svelte';
    import { goto } from '$app/navigation';
    import { page } from '$app/state';
    import { api } from '#lib/api.js';
    import ActionParamForm, {
        emptyForm,
        type AuthorForm,
    } from '#lib/components/ActionParamForm.svelte';
    import Badge from '#lib/components/Badge.svelte';
    import ToonEditor from '#lib/components/ToonEditor.svelte';
    import type {
        CatalogAction,
        ScaffoldRequest,
        ScaffoldResponse,
        ValidateToonResponse,
    } from '#lib/types.js';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';

    const plugin = $derived(page.url.searchParams.get('plugin') ?? '');
    const actionName = $derived(page.url.searchParams.get('action') ?? '');

    let action = $state<CatalogAction | null>(null);
    let loadError = $state<string | null>(null);

    let form: AuthorForm = $state(emptyForm());
    let toon = $state('');
    let scaffoldResult = $state<ScaffoldResponse | null>(null);
    let scaffolding = $state(false);
    let scaffoldError = $state<string | null>(null);
    let registered = $state<ValidateToonResponse | null>(null);

    onMount(() => {
        api.catalog()
            .then((c) => {
                const found = c.domains
                    .flatMap((d) => d.actions)
                    .find((a) => a.plugin === plugin && a.name === actionName);
                if (found) {
                    action = found;
                } else {
                    loadError = `unknown action ${plugin}::${actionName}`;
                }
            })
            .catch((e) => (loadError = String(e)));
    });

    const missingRequired = $derived(
        form.target.trim() === '' ||
            (action?.args ?? []).some((a) => a.required && (form.args[a.name] ?? '').trim() === ''),
    );

    async function generate() {
        if (!action || missingRequired || scaffolding) return;
        scaffolding = true;
        scaffoldError = null;
        registered = null;
        try {
            const args = Object.fromEntries(
                Object.entries(form.args).filter(([, v]) => v.trim() !== ''),
            );
            const req: ScaffoldRequest = {
                plugin: action.plugin,
                action: action.name,
                args,
                target: form.target.trim(),
            };
            if (form.title.trim() !== '') req.title = form.title.trim();
            if (form.probeKind === 'http' && form.probeUrl.trim() !== '') {
                req.probe_url = form.probeUrl.trim();
            } else if (form.probeKind === 'exec' && form.probeCommand.trim() !== '') {
                req.probe_command = form.probeCommand.trim();
            }
            if (form.probeExpect.trim() !== '') req.probe_expect = form.probeExpect.trim();
            scaffoldResult = await api.scaffold(req);
            toon = scaffoldResult.toon;
        } catch (e) {
            scaffoldResult = null;
            scaffoldError = String(e);
        } finally {
            scaffolding = false;
        }
    }
</script>

<PageHeader title="Author an experiment" back={{ href: '/author', label: 'catalog' }} />

{#if loadError}
    <State kind="error">Failed to load the action: {loadError}</State>
{:else if !action}
    <State kind="loading" height={160} />
{:else}
    <Panel title="1 · Action" style="margin-bottom: var(--space-4)">
        <div class="action-line">
            <span class="mono">{action.plugin}::{action.name}</span>
            <Badge kind="catalog" value={action.kind} />
        </div>
        <p class="desc">{action.description}</p>
    </Panel>

    <Panel title="2 · Parameters" style="margin-bottom: var(--space-4)">
        <ActionParamForm {action} bind:form />
        {#if missingRequired}
            <div class="hint">Target and all required arguments are needed before generating.</div>
        {/if}
    </Panel>

    <Panel title="3 · Experiment TOON" style="margin-bottom: var(--space-4)">
        <button class="primary" onclick={generate} disabled={missingRequired || scaffolding}>
            {scaffolding ? 'Generating…' : toon ? 'Regenerate TOON' : 'Generate TOON'}
        </button>
        {#if scaffoldError}
            <State kind="error">Scaffold failed: {scaffoldError}</State>
        {:else if scaffoldResult && !scaffoldResult.valid}
            <State kind="error">
                The generated experiment does not validate: {scaffoldResult.validation_error ??
                    'unknown error'}
                — edit the TOON below to fix it.
            </State>
        {/if}
        {#if toon}
            <div style="margin-top: var(--space-3)">
                <ToonEditor bind:toon bind:result={registered} />
            </div>
        {/if}
    </Panel>

    {#if registered && registered.valid}
        <Panel title="4 · Launch">
            <p class="desc">
                <span class="mono">{registered.registry_id}</span> is registered — dry-run the resolved
                plan and start the run from the launcher.
            </p>
            <button
                class="primary"
                onclick={() =>
                    goto(
                        `/runs/new?registry_id=${encodeURIComponent(registered?.valid ? registered.registry_id : '')}`,
                    )}
            >
                Dry run & launch →
            </button>
        </Panel>
    {/if}
{/if}

<style>
    .action-line {
        display: flex;
        align-items: center;
        gap: var(--space-3);
    }
    .desc {
        color: var(--text-dim);
        font-size: var(--font-size-md);
        margin: var(--space-2) 0;
    }
</style>

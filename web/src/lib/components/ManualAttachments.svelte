<script lang="ts">
    // The evidence of a record: files with their type, size and SHA-256, and
    // links. A file is taken in any status (the attach is an audit row and
    // leaves the seal as it was); a link only while the record is a draft or
    // submitted, as the API takes it.
    import { api, fmtTs } from '#lib/api.js';
    import { errorText, fmtBytes } from '#lib/manual.js';
    import type { EvidenceAttachment } from '#lib/types.js';

    let {
        experimentId,
        attachments,
        status,
        actor,
        canWrite,
        onchange,
    }: {
        experimentId: string;
        attachments: EvidenceAttachment[];
        status: string;
        actor: string;
        canWrite: boolean;
        onchange?: () => void;
    } = $props();

    let file: File | null = $state(null);
    let fileInput: HTMLInputElement | undefined = $state();
    let linkKind = $state('url');
    let linkUri = $state('');
    let linkLabel = $state('');
    let busy = $state(false);
    let message: { ok: boolean; text: string } | null = $state(null);

    const takesLinks = $derived(status === 'draft' || status === 'submitted');

    async function act(work: () => Promise<string>) {
        busy = true;
        message = null;
        try {
            message = { ok: true, text: await work() };
            onchange?.();
        } catch (e) {
            message = { ok: false, text: errorText(e) };
        } finally {
            busy = false;
        }
    }

    function upload() {
        const chosen = file;
        if (!chosen) return;
        void act(async () => {
            const added = await api.manualAttachFile(experimentId, chosen, actor);
            file = null;
            if (fileInput) fileInput.value = '';
            return `Attached ${added.label}: ${added.media_type}, ${fmtBytes(added.size_bytes)}.`;
        });
    }

    function addLink() {
        void act(async () => {
            await api.manualAttachLink(experimentId, {
                kind: linkKind,
                uri: linkUri.trim(),
                label: linkLabel.trim() || null,
                added_by: actor.trim(),
            });
            linkUri = '';
            linkLabel = '';
            return 'Link added.';
        });
    }

    async function download(at: EvidenceAttachment) {
        message = null;
        try {
            const blob = await api.manualDownload(experimentId, at.id);
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = at.label ?? 'attachment';
            document.body.append(a);
            a.click();
            a.remove();
            // The browser reads the address after the click returns.
            setTimeout(() => URL.revokeObjectURL(url), 10_000);
        } catch (e) {
            message = { ok: false, text: errorText(e) };
        }
    }
</script>

{#if attachments.length === 0}
    <div class="dim">No evidence attached yet.</div>
{:else}
    <table class="data evidence">
        <thead>
            <tr><th>Evidence</th><th>Type</th><th>Size</th><th>SHA-256</th><th></th></tr>
        </thead>
        <tbody>
            {#each attachments as at (at.id)}
                <tr>
                    <td>
                        <span class="badge neutral">{at.kind}</span>
                        {#if at.kind === 'file'}
                            {at.label ?? 'attachment'}
                        {:else}
                            <a href={at.uri} target="_blank" rel="noreferrer"
                                >{at.label ?? at.uri}</a
                            >
                        {/if}
                        <div class="dim">added by {at.added_by} · {fmtTs(at.added_at_ns)}</div>
                    </td>
                    <td class="mono nowrap">{at.media_type ?? ''}</td>
                    <td class="mono nowrap">{at.kind === 'file' ? fmtBytes(at.size_bytes) : ''}</td>
                    <td class="mono nowrap" title={at.file_hash ? `SHA-256 ${at.file_hash}` : null}
                        >{at.file_hash ? `${at.file_hash.slice(0, 12)}…` : ''}</td
                    >
                    <td>
                        {#if at.kind === 'file'}
                            <button
                                class="btn"
                                aria-label="Download {at.label ?? 'attachment'}"
                                onclick={() => download(at)}>Download</button
                            >
                        {/if}
                    </td>
                </tr>
            {/each}
        </tbody>
    </table>
{/if}

{#if canWrite}
    <div class="attach">
        <label
            >File to attach
            <input
                type="file"
                bind:this={fileInput}
                onchange={(e) => (file = e.currentTarget.files?.[0] ?? null)}
            />
        </label>
        <button class="btn" disabled={busy || !file} onclick={upload}>Upload file</button>
        <span class="dim">PDF, PNG, JPEG, text, CSV or JSON, at most 25 MB.</span>
    </div>
    {#if takesLinks}
        <div class="attach">
            <label
                >Link kind
                <select bind:value={linkKind}>
                    <option value="url">url</option>
                    <option value="ticket">ticket</option>
                </select>
            </label>
            <label class="grow">Link address <input type="text" bind:value={linkUri} /></label>
            <label>Link label <input type="text" bind:value={linkLabel} /></label>
            <button class="btn" disabled={busy} onclick={addLink}>Add link</button>
        </div>
    {/if}
{/if}
{#if message}
    <div class="state" class:error={!message.ok} role="status">{message.text}</div>
{/if}

<style>
    .attach {
        display: flex;
        gap: 10px;
        align-items: flex-end;
        flex-wrap: wrap;
        margin-top: 10px;
    }
    .attach label {
        display: flex;
        flex-direction: column;
        gap: 4px;
        font-size: 12px;
        color: var(--text-dim);
    }
    .attach .grow {
        flex: 1 1 220px;
    }
    .dim {
        color: var(--text-faint);
        font-size: 12px;
    }
    .nowrap {
        white-space: nowrap;
    }
</style>

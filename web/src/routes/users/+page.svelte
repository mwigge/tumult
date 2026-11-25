<script lang="ts">
    import { onMount } from 'svelte';
    import { api, fmtAgo, fmtTs } from '#lib/api.js';
    import type { AdminUser, ApiToken, MeResponse, Role } from '#lib/types.js';
    import Badge from '#lib/components/Badge.svelte';
    import PageHeader from '#lib/components/PageHeader.svelte';
    import Panel from '#lib/components/Panel.svelte';
    import State from '#lib/components/State.svelte';
    import { onRow } from '#lib/rows.js';

    const ROLES: Role[] = ['viewer', 'operator', 'approver', 'admin'];

    let me = $state<MeResponse | null>(null);
    let meError = $state<string | null>(null);
    let users = $state<AdminUser[] | null>(null);
    let error = $state<string | null>(null);

    // API tokens (admin list; hashes are never exposed).
    let tokens = $state<ApiToken[] | null>(null);
    let tokenName = $state('');
    let tokenUserId = $state('');
    let tokenDays = $state('');
    let tokenError = $state<string | null>(null);
    // Shown once after minting — the plaintext token is never retrievable again.
    let minted = $state<{ name: string; token: string } | null>(null);
    let minting = $state(false);

    // One row expands at a time; the editors below belong to that row.
    let openId = $state<string | null>(null);
    let roleEdit = $state<Role>('viewer');
    let scopeEdit = $state('');
    let passwordEdit = $state('');
    let rowError = $state<string | null>(null);
    let busy = $state(false);

    // Create-user form.
    let newUsername = $state('');
    let newRole = $state<Role>('viewer');
    let newPassword = $state('');
    let newScopes = $state('');
    let createError = $state<string | null>(null);
    // Shown once after creating a user without a supplied password — the
    // one-time password is never retrievable again.
    let oneTime = $state<{ username: string; password: string } | null>(null);
    let creating = $state(false);

    // Admins only (the server enforces it with 403 regardless); open local mode
    // (no users yet) lets anyone in — that is how the first admin is created.
    const isAdmin = $derived(me ? !me.auth_required || me.role === 'admin' : false);

    onMount(() => {
        api.me()
            .then((m) => (me = m))
            .catch((e) => (meError = String(e)));
    });

    // Load the user list once the caller is known to be an admin (or once open
    // local mode is known); runs once, guarded by `users !== null`.
    $effect(() => {
        if (!isAdmin || users !== null) return;
        refresh();
    });

    async function refresh() {
        try {
            const r = await api.users();
            users = r.users;
            error = null;
        } catch (e) {
            // Always surfaced: with a list already loaded (e.g. after a role change
            // dropped the caller's own admin rights) the banner explains why the
            // page's actions now fail; the last good list stays visible.
            error = String(e);
        }
        try {
            const r = await api.tokens();
            tokens = r.tokens;
        } catch (e) {
            if (!tokens) tokenError = String(e);
        }
    }

    function toggle(u: AdminUser) {
        if (openId === u.id) {
            openId = null;
            return;
        }
        openId = u.id;
        roleEdit = u.role;
        scopeEdit = u.env_scopes.join(', ');
        passwordEdit = '';
        rowError = null;
    }

    /** Comma-separated scopes text → list; empty means every environment. */
    function parseScopes(raw: string): string[] {
        return raw
            .split(',')
            .map((s) => s.trim())
            .filter((s) => s !== '');
    }

    /** Shared row-action wrapper: busy guard + error capture on the open row. */
    async function act(fn: () => Promise<unknown>) {
        if (busy) return;
        busy = true;
        rowError = null;
        try {
            await fn();
            await refresh();
        } catch (e) {
            rowError = String(e);
        } finally {
            busy = false;
        }
    }

    const saveRole = (u: AdminUser) => act(() => api.setUserRole(u.id, roleEdit));
    const saveScopes = (u: AdminUser) => act(() => api.setUserScopes(u.id, parseScopes(scopeEdit)));
    const savePassword = (u: AdminUser) =>
        act(async () => {
            await api.resetUserPassword(u.id, passwordEdit);
            passwordEdit = '';
        });
    const toggleDisabled = (u: AdminUser) => act(() => api.setUserDisabled(u.id, !u.disabled));

    async function create() {
        if (creating) return;
        creating = true;
        createError = null;
        oneTime = null;
        try {
            const r = await api.createUser({
                username: newUsername.trim(),
                role: newRole,
                ...(newPassword ? { password: newPassword } : {}),
                env_scopes: parseScopes(newScopes),
            });
            if (r.one_time_password) {
                oneTime = { username: r.username, password: r.one_time_password };
            }
            newUsername = '';
            newPassword = '';
            newScopes = '';
            newRole = 'viewer';
            await refresh();
        } catch (e) {
            createError = String(e);
        } finally {
            creating = false;
        }
    }

    /** The state of a token: revoked and expired are terminal; else active. */
    function tokenStatus(t: ApiToken): string {
        if (t.revoked) return 'revoked';
        if (t.expires_at_ns !== null && t.expires_at_ns / 1_000_000 <= Date.now()) return 'expired';
        return 'active';
    }

    async function mint() {
        if (minting) return;
        minting = true;
        tokenError = null;
        minted = null;
        try {
            const days = Number(tokenDays);
            const r = await api.createToken({
                name: tokenName.trim(),
                ...(tokenUserId ? { user_id: tokenUserId } : {}),
                ...(tokenDays && days > 0
                    ? { expires_at_ns: (Date.now() + days * 86_400_000) * 1_000_000 }
                    : {}),
            });
            minted = { name: tokenName.trim(), token: r.token };
            tokenName = '';
            tokenDays = '';
            await refresh();
        } catch (e) {
            tokenError = String(e);
        } finally {
            minting = false;
        }
    }

    async function revoke(t: ApiToken) {
        if (busy) return;
        busy = true;
        tokenError = null;
        try {
            await api.revokeToken(t.id);
            await refresh();
        } catch (e) {
            tokenError = String(e);
        } finally {
            busy = false;
        }
    }
</script>

<PageHeader
    title="Users"
    sub={users
        ? `${users.length} account${users.length === 1 ? '' : 's'}`
        : 'accounts, roles and scopes'}
/>

{#if meError}
    <Panel>
        <State kind="error">Failed to load the session: {meError}</State>
    </Panel>
{:else if me && !isAdmin}
    <Panel>
        <State kind="empty">
            User management requires the admin role{me.role ? ` (you are ${me.role})` : ''}.
        </State>
    </Panel>
{:else}
    <Panel style="margin-bottom: var(--space-4)" scroll>
        {#if !users}
            {#if error}
                <State kind="error">Failed to load users: {error}</State>
            {:else}
                <State kind="loading" height={160} />
            {/if}
        {:else if users.length === 0}
            <State kind="empty">
                No users yet — the daemon is in open local mode. Create the first admin below (or
                with
                <span class="mono">tumultd create-admin</span>) to turn authentication on.
            </State>
        {:else}
            {#if error}
                <State kind="error" style="margin-bottom: var(--space-2)">
                    Refresh failed: {error} — showing the last loaded list.
                </State>
            {/if}
            <table class="data">
                <thead>
                    <tr>
                        <th></th><th>Username</th><th>Role</th><th>Scopes</th><th>Status</th><th
                            >Created</th
                        >
                    </tr>
                </thead>
                <tbody>
                    {#each users as u (u.id)}
                        <tr
                            class="clickable"
                            class:selected={openId === u.id}
                            onclick={onRow(() => toggle(u))}
                        >
                            <td style="color: var(--text-faint)">{openId === u.id ? '▾' : '▸'}</td>
                            <td>
                                <button
                                    class="row-button"
                                    aria-expanded={openId === u.id}
                                    onclick={() => toggle(u)}>{u.username}</button
                                >
                                {#if me?.username === u.username}<Badge
                                        tone="neutral"
                                        label="you"
                                    />{/if}
                            </td>
                            <td><Badge tone="neutral" label={u.role} /></td>
                            <td
                                >{u.env_scopes.length === 0
                                    ? 'all environments'
                                    : u.env_scopes.join(', ')}</td
                            >
                            <td>
                                {#if u.disabled}<Badge tone="fail" label="disabled" />{/if}
                                {#if u.must_change}<Badge
                                        tone="warn"
                                        label="must change password"
                                    />{/if}
                                {#if !u.disabled && !u.must_change}<Badge
                                        tone="ok"
                                        label="active"
                                    />{/if}
                            </td>
                            <td title={fmtTs(u.created_at_ns)}>{fmtAgo(u.created_at_ns)}</td>
                        </tr>
                        {#if openId === u.id}
                            <tr class="detail-row">
                                <td colspan={6}>
                                    <div class="detail">
                                        <div>
                                            <h2>Role</h2>
                                            {#if me?.username === u.username}
                                                <div class="hint">
                                                    You cannot change your own role — ask another
                                                    admin.
                                                </div>
                                            {:else}
                                                <div class="row">
                                                    <select aria-label="Role" bind:value={roleEdit}>
                                                        {#each ROLES as r (r)}<option value={r}
                                                                >{r}</option
                                                            >{/each}
                                                    </select>
                                                    <button
                                                        class="btn"
                                                        onclick={() => saveRole(u)}
                                                        disabled={busy || roleEdit === u.role}
                                                        >Save</button
                                                    >
                                                </div>
                                            {/if}
                                            <h2>Environment scopes</h2>
                                            <div class="row">
                                                <input
                                                    type="text"
                                                    aria-label="Environment scopes"
                                                    placeholder="comma-separated; empty = all"
                                                    bind:value={scopeEdit}
                                                />
                                                <button
                                                    class="btn"
                                                    onclick={() => saveScopes(u)}
                                                    disabled={busy}>Save</button
                                                >
                                            </div>
                                        </div>
                                        <div>
                                            <h2>Reset password</h2>
                                            <div class="row">
                                                <input
                                                    type="password"
                                                    aria-label="New one-time password"
                                                    placeholder="new one-time password (min 12 chars)"
                                                    autocomplete="new-password"
                                                    bind:value={passwordEdit}
                                                />
                                                <button
                                                    class="btn"
                                                    onclick={() => savePassword(u)}
                                                    disabled={busy || passwordEdit.length < 12}
                                                    >Reset</button
                                                >
                                            </div>
                                            <div class="hint">
                                                The user must change it at next login.
                                            </div>
                                            <h2>Account</h2>
                                            {#if me?.username === u.username}
                                                <div class="hint">
                                                    You cannot disable your own account.
                                                </div>
                                            {:else}
                                                <button
                                                    class="btn danger"
                                                    onclick={() => toggleDisabled(u)}
                                                    disabled={busy}
                                                >
                                                    {u.disabled
                                                        ? 'Re-enable account'
                                                        : 'Disable account'}
                                                </button>
                                            {/if}
                                        </div>
                                    </div>
                                    {#if rowError}<div class="err-text">{rowError}</div>{/if}
                                </td>
                            </tr>
                        {/if}
                    {/each}
                </tbody>
            </table>
        {/if}
    </Panel>

    <Panel title="Create user">
        <form
            class="create"
            onsubmit={(e) => {
                e.preventDefault();
                create();
            }}
        >
            <label>
                Username
                <input type="text" bind:value={newUsername} autocomplete="off" required />
            </label>
            <label>
                Role
                <select bind:value={newRole}>
                    {#each ROLES as r (r)}<option value={r}>{r}</option>{/each}
                </select>
            </label>
            <label>
                Password (optional)
                <input
                    type="password"
                    bind:value={newPassword}
                    autocomplete="new-password"
                    placeholder="empty = generate one-time"
                />
                <span class="hint">12+ characters if set; empty generates a one-time password.</span
                >
            </label>
            <label>
                Environment scopes (optional)
                <input
                    type="text"
                    bind:value={newScopes}
                    placeholder="comma-separated; empty = all"
                />
            </label>
            <div>
                <button
                    class="primary"
                    type="submit"
                    disabled={creating ||
                        !newUsername.trim() ||
                        (newPassword.length > 0 && newPassword.length < 12)}
                >
                    {creating ? 'Creating…' : 'Create user'}
                </button>
            </div>
        </form>
        {#if createError}<State kind="error" style="margin-top: var(--space-3)">
                {createError}
            </State>{/if}
        {#if oneTime}
            <div class="onetime">
                One-time password for <b>{oneTime.username}</b> — shown once, never stored; the user
                must change it at first login:
                <div class="mono pw">{oneTime.password}</div>
            </div>
        {/if}
    </Panel>

    <Panel title="API tokens" scroll>
        {#if tokens === null}
            {#if tokenError}
                <State kind="error">Failed to load tokens: {tokenError}</State>
            {:else}
                <State kind="loading" height={80} />
            {/if}
        {:else}
            {#if tokens.length > 0}
                <table class="data" style="margin-bottom: var(--space-4)">
                    <thead>
                        <tr
                            ><th>Name</th><th>Owner</th><th>Created</th><th>Last used</th><th
                                >Expires</th
                            ><th>Status</th><th></th></tr
                        >
                    </thead>
                    <tbody>
                        {#each tokens as t (t.id)}
                            {@const st = tokenStatus(t)}
                            <tr>
                                <td>{t.name}</td>
                                <td>{t.username ?? '—'}</td>
                                <td title={fmtTs(t.created_at_ns)}>{fmtAgo(t.created_at_ns)}</td>
                                <td
                                    title={t.last_used_at_ns !== null
                                        ? fmtTs(t.last_used_at_ns)
                                        : ''}
                                >
                                    {t.last_used_at_ns !== null
                                        ? fmtAgo(t.last_used_at_ns)
                                        : 'never'}
                                </td>
                                <td title={t.expires_at_ns !== null ? fmtTs(t.expires_at_ns) : ''}>
                                    {t.expires_at_ns !== null ? fmtTs(t.expires_at_ns) : 'never'}
                                </td>
                                <td><Badge kind="token" value={st} /></td>
                                <td>
                                    {#if !t.revoked}
                                        <button
                                            class="btn danger"
                                            onclick={() => revoke(t)}
                                            disabled={busy}>Revoke</button
                                        >
                                    {/if}
                                </td>
                            </tr>
                        {/each}
                    </tbody>
                </table>
            {/if}
            <form
                class="create"
                onsubmit={(e) => {
                    e.preventDefault();
                    mint();
                }}
            >
                <label>
                    Name
                    <input
                        type="text"
                        bind:value={tokenName}
                        placeholder="e.g. deploy script"
                        autocomplete="off"
                        required
                    />
                </label>
                <label>
                    Owner
                    <select bind:value={tokenUserId}>
                        <option value="">yourself ({me?.username ?? 'current user'})</option>
                        {#each users ?? [] as u (u.id)}
                            <option value={u.id}>{u.username}</option>
                        {/each}
                    </select>
                </label>
                <label>
                    Expires in days (optional)
                    <input type="number" min="1" bind:value={tokenDays} placeholder="never" />
                </label>
                <div>
                    <button class="primary" type="submit" disabled={minting || !tokenName.trim()}>
                        {minting ? 'Minting…' : 'Mint token'}
                    </button>
                </div>
            </form>
            {#if tokenError && tokens !== null}<State
                    kind="error"
                    style="margin-top: var(--space-3)"
                >
                    {tokenError}
                </State>{/if}
            {#if minted}
                <div class="onetime">
                    Token <b>{minted.name}</b> — shown once, never stored:
                    <div class="mono pw">{minted.token}</div>
                </div>
            {/if}
        {/if}
    </Panel>
{/if}

<style>
    tr.selected td {
        background: var(--bg-hover);
    }
    .detail {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: var(--space-4);
        padding: var(--space-2) var(--space-1);
        cursor: default;
    }
    .detail h2 {
        font-size: var(--font-size-xs);
        text-transform: uppercase;
        letter-spacing: var(--letter-spacing-sm);
        color: var(--text-dim);
        margin: 0 0 var(--space-2);
    }
    .detail h2:not(:first-child) {
        margin-top: var(--space-3);
    }
    .row {
        display: flex;
        gap: var(--space-2);
        align-items: center;
    }
    .row input,
    .row select {
        flex: 1;
        min-width: 0;
    }
    .btn {
        white-space: nowrap;
    }
    .hint {
        color: var(--text-faint);
        font-size: var(--font-size-xs);
        margin-top: var(--space-1);
    }
    .err-text {
        color: var(--fail);
        font-size: var(--font-size-xs);
        padding: 0 var(--space-1) var(--space-2);
    }
    .create {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
        gap: var(--space-3) var(--space-5);
        align-items: end;
    }
    .create label {
        display: flex;
        flex-direction: column;
        gap: var(--space-1);
        font-size: var(--font-size-xs);
        color: var(--text-dim);
    }
    .onetime {
        margin-top: var(--space-4);
        border: var(--border-width-sm) solid var(--warn);
        border-radius: var(--radius-xl);
        padding: var(--space-3);
        font-size: var(--font-size-md);
    }
    .pw {
        font-size: var(--font-size-xl);
        user-select: all;
        margin-top: var(--space-2);
    }
</style>

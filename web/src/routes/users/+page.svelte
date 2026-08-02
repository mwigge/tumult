<script lang="ts">
  import { onMount } from 'svelte';
  import { api, fmtAgo, fmtTs } from '$lib/api';
  import type { AdminUser, ApiToken, MeResponse, Role } from '$lib/types';

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
    api
      .me()
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
  const saveScopes = (u: AdminUser) =>
    act(() => api.setUserScopes(u.id, parseScopes(scopeEdit)));
  const savePassword = (u: AdminUser) =>
    act(async () => {
      await api.resetUserPassword(u.id, passwordEdit);
      passwordEdit = '';
    });
  const toggleDisabled = (u: AdminUser) =>
    act(() => api.setUserDisabled(u.id, !u.disabled));

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
        env_scopes: parseScopes(newScopes)
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

  /** Token status badge: revoked and expired are terminal; else active. */
  function tokenStatus(t: ApiToken): { label: string; cls: string } {
    if (t.revoked) return { label: 'revoked', cls: 'fail' };
    if (t.expires_at_ns !== null && t.expires_at_ns / 1_000_000 <= Date.now()) {
      return { label: 'expired', cls: 'warn' };
    }
    return { label: 'active', cls: 'ok' };
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
          : {})
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

<div class="page-head">
  <h1>Users</h1>
  <span class="sub">
    {users ? `${users.length} account${users.length === 1 ? '' : 's'}` : 'accounts, roles and scopes'}
  </span>
</div>

{#if meError}
  <div class="panel">
    <div class="state error">Failed to load the session: {meError}</div>
  </div>
{:else if me && !isAdmin}
  <div class="panel">
    <div class="state">
      User management requires the admin role{me.role ? ` (you are ${me.role})` : ''}.
    </div>
  </div>
{:else}
  <div class="panel" style="margin-bottom: 14px">
    {#if !users}
      {#if error}
        <div class="state error">Failed to load users: {error}</div>
      {:else}
        <div class="skeleton" style="height: 160px"></div>
      {/if}
    {:else if users.length === 0}
      <div class="state">
        No users yet — the daemon is in open local mode. Create the first admin below (or with
        <span class="mono">tumultd create-admin</span>) to turn authentication on.
      </div>
    {:else}
      {#if error}
        <div class="state error" style="margin-bottom: 8px">
          Refresh failed: {error} — showing the last loaded list.
        </div>
      {/if}
      <table class="data">
        <thead>
          <tr>
            <th></th><th>Username</th><th>Role</th><th>Scopes</th><th>Status</th><th>Created</th>
          </tr>
        </thead>
        <tbody>
          {#each users as u (u.id)}
            <tr class="clickable" class:selected={openId === u.id} onclick={() => toggle(u)}>
              <td style="color: var(--text-faint)">{openId === u.id ? '▾' : '▸'}</td>
              <td>
                {u.username}
                {#if me?.username === u.username}<span class="badge neutral">you</span>{/if}
              </td>
              <td><span class="badge neutral">{u.role}</span></td>
              <td>{u.env_scopes.length === 0 ? 'all environments' : u.env_scopes.join(', ')}</td>
              <td>
                {#if u.disabled}<span class="badge fail">disabled</span>{/if}
                {#if u.must_change}<span class="badge warn">must change password</span>{/if}
                {#if !u.disabled && !u.must_change}<span class="badge ok">active</span>{/if}
              </td>
              <td title={fmtTs(u.created_at_ns)}>{fmtAgo(u.created_at_ns)}</td>
            </tr>
            {#if openId === u.id}
              <tr class="detail-row">
                <td colspan={6}>
                  <div class="detail">
                    <div>
                      <h3>Role</h3>
                      {#if me?.username === u.username}
                        <div class="hint">You cannot change your own role — ask another admin.</div>
                      {:else}
                        <div class="row">
                          <select bind:value={roleEdit}>
                            {#each ROLES as r (r)}<option value={r}>{r}</option>{/each}
                          </select>
                          <button class="btn" onclick={() => saveRole(u)} disabled={busy || roleEdit === u.role}>Save</button>
                        </div>
                      {/if}
                      <h3>Environment scopes</h3>
                      <div class="row">
                        <input type="text" placeholder="comma-separated; empty = all" bind:value={scopeEdit} />
                        <button class="btn" onclick={() => saveScopes(u)} disabled={busy}>Save</button>
                      </div>
                    </div>
                    <div>
                      <h3>Reset password</h3>
                      <div class="row">
                        <input type="password" placeholder="new one-time password (min 12 chars)" autocomplete="new-password" bind:value={passwordEdit} />
                        <button class="btn" onclick={() => savePassword(u)} disabled={busy || passwordEdit.length < 12}>Reset</button>
                      </div>
                      <div class="hint">The user must change it at next login.</div>
                      <h3>Account</h3>
                      {#if me?.username === u.username}
                        <div class="hint">You cannot disable your own account.</div>
                      {:else}
                        <button class="btn danger" onclick={() => toggleDisabled(u)} disabled={busy}>
                          {u.disabled ? 'Re-enable account' : 'Disable account'}
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
  </div>

  <div class="panel">
    <h2>Create user</h2>
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
        <input type="password" bind:value={newPassword} autocomplete="new-password" placeholder="empty = generate one-time" />
        <span class="hint">12+ characters if set; empty generates a one-time password.</span>
      </label>
      <label>
        Environment scopes (optional)
        <input type="text" bind:value={newScopes} placeholder="comma-separated; empty = all" />
      </label>
      <div>
        <button
          class="primary"
          type="submit"
          disabled={creating || !newUsername.trim() || (newPassword.length > 0 && newPassword.length < 12)}
        >
          {creating ? 'Creating…' : 'Create user'}
        </button>
      </div>
    </form>
    {#if createError}<div class="state error" style="margin-top: 10px">{createError}</div>{/if}
    {#if oneTime}
      <div class="onetime">
        One-time password for <b>{oneTime.username}</b> — shown once, never stored;
        the user must change it at first login:
        <div class="mono pw">{oneTime.password}</div>
      </div>
    {/if}
  </div>

  <div class="panel">
    <h2>API tokens</h2>
    {#if tokens === null}
      {#if tokenError}
        <div class="state error">Failed to load tokens: {tokenError}</div>
      {:else}
        <div class="skeleton" style="height: 80px"></div>
      {/if}
    {:else}
      {#if tokens.length > 0}
        <table class="data" style="margin-bottom: 14px">
          <thead>
            <tr><th>Name</th><th>Owner</th><th>Created</th><th>Last used</th><th>Expires</th><th>Status</th><th></th></tr>
          </thead>
          <tbody>
            {#each tokens as t (t.id)}
              {@const st = tokenStatus(t)}
              <tr>
                <td>{t.name}</td>
                <td>{t.username ?? '—'}</td>
                <td title={fmtTs(t.created_at_ns)}>{fmtAgo(t.created_at_ns)}</td>
                <td title={t.last_used_at_ns !== null ? fmtTs(t.last_used_at_ns) : ''}>
                  {t.last_used_at_ns !== null ? fmtAgo(t.last_used_at_ns) : 'never'}
                </td>
                <td title={t.expires_at_ns !== null ? fmtTs(t.expires_at_ns) : ''}>
                  {t.expires_at_ns !== null ? fmtTs(t.expires_at_ns) : 'never'}
                </td>
                <td><span class="badge {st.cls}">{st.label}</span></td>
                <td>
                  {#if !t.revoked}
                    <button class="btn danger" onclick={() => revoke(t)} disabled={busy}>Revoke</button>
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
          <input type="text" bind:value={tokenName} placeholder="e.g. deploy script" autocomplete="off" required />
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
      {#if tokenError && tokens !== null}<div class="state error" style="margin-top: 10px">{tokenError}</div>{/if}
      {#if minted}
        <div class="onetime">
          Token <b>{minted.name}</b> — shown once, never stored:
          <div class="mono pw">{minted.token}</div>
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  tr.selected td { background: var(--bg-hover); }
  .detail-row td { background: var(--bg-raised); }
  .detail {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
    padding: 8px 4px;
    cursor: default;
  }
  .detail h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-dim);
    margin: 0 0 6px;
  }
  .detail h3:not(:first-child) { margin-top: 12px; }
  .row { display: flex; gap: 8px; align-items: center; }
  .row input, .row select { flex: 1; min-width: 0; }
  .btn {
    background: var(--bg-hover);
    border: 1px solid var(--border-strong);
    color: var(--text);
    border-radius: 6px;
    padding: 6px 14px;
    cursor: pointer;
    font-size: 13px;
    white-space: nowrap;
  }
  .btn:hover { border-color: var(--accent-dim); }
  .btn.danger { border-color: var(--fail); color: var(--fail); }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .hint { color: var(--text-faint); font-size: 12px; margin-top: 4px; }
  .err-text { color: var(--fail); font-size: 12.5px; padding: 0 4px 6px; }
  .create {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 12px 18px;
    align-items: end;
  }
  .create label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  .onetime {
    margin-top: 14px;
    border: 1px solid var(--warn);
    border-radius: 6px;
    padding: 10px 12px;
    font-size: 13px;
  }
  .pw { font-size: 14px; user-select: all; margin-top: 6px; }
</style>

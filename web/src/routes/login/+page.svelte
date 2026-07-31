<script lang="ts">
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';

  let username = $state('');
  let password = $state('');
  let error: string | null = $state(null);
  let submitting = $state(false);

  // After a successful login with must_change, swap to the password form.
  // The one-time password just used stays in `password` (readonly field).
  let mustChange = $state(false);
  let newPassword = $state('');
  let confirmPassword = $state('');

  async function submitLogin() {
    if (submitting) return;
    submitting = true;
    error = null;
    try {
      const res = await api.login(username, password);
      if (res.must_change) {
        mustChange = true;
      } else {
        await goto('/');
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      submitting = false;
    }
  }

  async function submitChange() {
    if (submitting) return;
    if (newPassword !== confirmPassword) {
      error = 'Passwords do not match.';
      return;
    }
    submitting = true;
    error = null;
    try {
      await api.changePassword(password, newPassword);
      await goto('/');
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      submitting = false;
    }
  }
</script>

<div class="login-wrap">
  <div class="panel login-panel">
    <div class="brand">
      <div class="name">Tumult</div>
      <div class="tag">analytics for your resilience work</div>
    </div>

    {#if !mustChange}
      <form
        onsubmit={(e) => {
          e.preventDefault();
          submitLogin();
        }}
      >
        <label>
          Username
          <input type="text" bind:value={username} autocomplete="username" required />
        </label>
        <label>
          Password
          <input
            type="password"
            bind:value={password}
            autocomplete="current-password"
            required
          />
        </label>
        {#if error}
          <div class="state error" style="padding: 4px 0; text-align: left">{error}</div>
        {/if}
        <button class="primary" type="submit" disabled={submitting || !username || !password}>
          {submitting ? 'Signing in…' : 'Sign in'}
        </button>
      </form>
    {:else}
      <p class="hint">
        This account uses a one-time password. Set a new password to continue.
      </p>
      <form
        onsubmit={(e) => {
          e.preventDefault();
          submitChange();
        }}
      >
        <label>
          Current (one-time) password
          <input type="password" value={password} readonly />
        </label>
        <label>
          New password
          <input
            type="password"
            bind:value={newPassword}
            autocomplete="new-password"
            minlength="12"
            required
          />
        </label>
        <label>
          Confirm new password
          <input
            type="password"
            bind:value={confirmPassword}
            autocomplete="new-password"
            minlength="12"
            required
          />
        </label>
        <div class="hint">Minimum 12 characters.</div>
        {#if error}
          <div class="state error" style="padding: 4px 0; text-align: left">{error}</div>
        {/if}
        <button
          class="primary"
          type="submit"
          disabled={submitting || newPassword.length < 12 || !confirmPassword}
        >
          {submitting ? 'Saving…' : 'Set new password'}
        </button>
      </form>
    {/if}
  </div>
</div>

<style>
  .login-wrap {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .login-panel {
    width: 340px;
  }
  .login-panel .brand {
    padding: 0 0 12px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 16px;
  }
  .login-panel .brand .name {
    font-size: 17px;
    font-weight: 700;
    letter-spacing: 0.4px;
  }
  .login-panel .brand .tag {
    color: var(--text-faint);
    font-size: 11px;
    margin-top: 2px;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12.5px;
    color: var(--text-dim);
  }
  input[type='password'] {
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    color: var(--text);
    border-radius: 5px;
    padding: 6px 9px;
    font-size: 13px;
    outline: none;
  }
  input[readonly] {
    color: var(--text-faint);
  }
  .hint {
    color: var(--text-faint);
    font-size: 12px;
    margin: 0 0 12px;
  }
  form .hint {
    margin: -4px 0 0;
  }
</style>

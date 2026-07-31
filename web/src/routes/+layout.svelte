<script lang="ts">
  import '$lib/theme.css';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { api } from '$lib/api';
  import type { MeResponse } from '$lib/types';

  let { children } = $props();

  const NAV = [
    { href: '/', label: 'Overview', icon: '◧' },
    { href: '/scores', label: 'Scores', icon: '▦' },
    { href: '/experiments', label: 'Experiments', icon: '⚗' },
    { href: '/runs', label: 'Runs', icon: '▶' },
    { href: '/approvals', label: 'Approvals', icon: '✓' },
    { href: '/manual', label: 'Manual', icon: '✎' },
    { href: '/logs', label: 'Logs', icon: '≣' },
    { href: '/traces', label: 'Traces', icon: '⌁' },
    { href: '/metrics', label: 'Metrics', icon: '∿' },
    { href: '/topology', label: 'Topology', icon: '✳' },
    { href: '/ask', label: 'Ask', icon: '✦' },
    { href: '/reports', label: 'Reports', icon: '▤' }
  ];

  function active(pathname: string, href: string): boolean {
    return href === '/' ? pathname === '/' : pathname.startsWith(href);
  }

  // Session state for the user chip. me() never breaks the layout: any
  // failure just leaves the chip hidden. Refreshed on every navigation so
  // returning from /login picks up the new session.
  let me: MeResponse | null = $state(null);
  let loggingOut = $state(false);

  $effect(() => {
    const pathname = $page.url.pathname;
    if (pathname === '/login') return;
    api
      .me()
      .then((m) => (me = m))
      .catch(() => (me = null));
  });

  async function logout() {
    if (loggingOut) return;
    loggingOut = true;
    try {
      await api.logout();
    } catch {
      // Clear local state and head to /login regardless.
    } finally {
      me = null;
      loggingOut = false;
      await goto('/login');
    }
  }
</script>

{#if $page.url.pathname === '/login'}
  {@render children?.()}
{:else}
  <div class="shell">
    <aside class="side">
      <div class="brand">
        <div class="name">Krönika</div>
        <div class="tag">the chronicle of your resilience work</div>
      </div>
      <nav class="nav">
        {#each NAV as item (item.href)}
          <a href={item.href} class:active={active($page.url.pathname, item.href)}>
            <span>{item.icon}</span>{item.label}
          </a>
        {/each}
      </nav>
      {#if me?.auth_required && me.authenticated}
        <div class="user-chip">
          <div class="who">
            <span class="username">{me.username}</span>
            {#if me.role}
              <span class="badge neutral">{me.role}</span>
            {/if}
          </div>
          <button class="logout" onclick={logout} disabled={loggingOut}>
            {loggingOut ? '…' : 'Log out'}
          </button>
        </div>
      {/if}
    </aside>
    <main class="main">
      {@render children?.()}
    </main>
  </div>
{/if}

<style>
  .user-chip {
    margin-top: auto;
    padding: 12px 18px 0;
    border-top: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .user-chip .who {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .user-chip .username {
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .user-chip .logout {
    background: transparent;
    border: 1px solid var(--border-strong);
    color: var(--text-dim);
    border-radius: 5px;
    padding: 3px 10px;
    font-size: 12px;
    cursor: pointer;
  }
  .user-chip .logout:hover {
    color: var(--text);
    border-color: var(--accent);
  }
  .user-chip .logout:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>

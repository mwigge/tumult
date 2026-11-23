<script lang="ts">
    import '#lib/theme.css';
    import { goto } from '$app/navigation';
    import { page } from '$app/state';
    import { api } from '#lib/api.js';
    import { TAGLINE } from '#lib/brand.js';
    import type { MeResponse } from '#lib/types.js';

    let { children } = $props();

    // Theme: dark is the default; `light` forces light, `auto` follows the
    // OS preference. Persisted per browser; applied as data-theme on <html>
    // so lib/theme.css and the ECharts themes pick it up.
    type Theme = 'dark' | 'light' | 'auto';
    const THEMES: Theme[] = ['dark', 'light', 'auto'];
    const THEME_GLYPH: Record<Theme, string> = { dark: '◐', light: '○', auto: '◑' };
    let theme = $state<Theme>('dark');
    $effect(() => {
        try {
            const saved = localStorage.getItem('tumult.theme');
            if (saved === 'light' || saved === 'auto' || saved === 'dark') theme = saved;
        } catch {
            /* storage unavailable: stay dark */
        }
    });
    $effect(() => {
        document.documentElement.dataset.theme = theme;
        try {
            localStorage.setItem('tumult.theme', theme);
        } catch {
            /* storage unavailable */
        }
    });
    function cycleTheme() {
        theme = THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
    }

    const NAV: {
        href: string;
        label: string;
        icon: string;
        admin?: boolean;
        operator?: boolean;
    }[] = [
        { href: '/', label: 'Overview', icon: '◧' },
        { href: '/resilience', label: 'Resilience', icon: '◎' },
        { href: '/performance', label: 'Performance', icon: '◭' },
        { href: '/scores', label: 'Scores', icon: '▦' },
        { href: '/scores?view=dora', label: 'DORA', icon: '⚖' },
        { href: '/dr', label: 'Disaster recovery', icon: '⟲' },
        { href: '/experiments', label: 'Experiments', icon: '⚗' },
        { href: '/runs', label: 'Runs', icon: '▶' },
        { href: '/author', label: 'Author', icon: '✚' },
        { href: '/gamedays', label: 'GameDays', icon: '☷' },
        { href: '/approvals', label: 'Approvals', icon: '✓' },
        { href: '/manual', label: 'Manual', icon: '✎' },
        { href: '/logs', label: 'Logs', icon: '≣' },
        { href: '/traces', label: 'Traces', icon: '⌁' },
        { href: '/metrics', label: 'Metrics', icon: '∿' },
        { href: '/starmap', label: 'Star Map', icon: '✳' },
        { href: '/ask', label: 'Ask', icon: '✦' },
        { href: '/reports', label: 'Reports', icon: '▤' },
        { href: '/events', label: 'Events', icon: '◔' },
        { href: '/users', label: 'Users', icon: '⚿', admin: true },
        { href: '/webhooks', label: 'Webhooks', icon: '➤', admin: true },
        { href: '/schedules', label: 'Schedules', icon: '↻', operator: true },
    ];

    // An entry with a query (`/scores?view=dora`) is active only when the
    // page carries that `view`; its plain sibling (`/scores`) only when not.
    function active(
        url: { pathname: string; searchParams: { get(name: string): string | null } },
        href: string,
    ): boolean {
        const [path, query] = href.split('?');
        if (path === '/') return url.pathname === '/';
        if (!url.pathname.startsWith(path)) return false;
        const wanted = new URLSearchParams(query ?? '').get('view');
        const viewed = url.searchParams.get('view');
        if (wanted) return viewed === wanted;
        return !NAV.some((n) => n.href === `${path}?view=${viewed}`);
    }

    // Session state for the user chip. me() never breaks the layout: any
    // failure just leaves the chip hidden. Refreshed on every navigation so
    // returning from /login picks up the new session.
    let me = $state<MeResponse | null>(null);
    let loggingOut = $state(false);

    // Admin-flagged nav entries show for admins; open local mode (no users,
    // `auth_required: false`) shows everything.
    const isAdmin = $derived(me ? !me.auth_required || me.role === 'admin' : false);
    // Operator-flagged entries show for operators and above (approver, admin).
    const isOperator = $derived(
        me ? !me.auth_required || (!!me.role && me.role !== 'viewer') : false,
    );

    $effect(() => {
        const pathname = page.url.pathname;
        if (pathname === '/login') return;
        api.me()
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

{#if page.url.pathname === '/login'}
    {@render children?.()}
{:else}
    <div class="shell">
        <aside class="side">
            <div class="brand">
                <div class="name">Tumult</div>
                <div class="tag">{TAGLINE}</div>
            </div>
            <nav class="nav">
                {#each NAV as item (item.href)}
                    {#if (!item.admin || isAdmin) && (!item.operator || isOperator)}
                        <a href={item.href} class:active={active(page.url, item.href)}>
                            <span>{item.icon}</span>{item.label}
                        </a>
                    {/if}
                {/each}
            </nav>
            <button class="theme" onclick={cycleTheme} title="theme: {theme} (click to change)">
                {THEME_GLYPH[theme]}
                {theme}
            </button>
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
    .theme {
        margin: 8px 18px 0;
        align-self: flex-start;
        background: none;
        border: 1px solid var(--border);
        border-radius: 4px;
        color: var(--text-dim);
        font: inherit;
        font-size: 11.5px;
        padding: 2px 8px;
        cursor: pointer;
    }
    .theme:hover {
        color: var(--text);
        border-color: var(--border-strong);
    }
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

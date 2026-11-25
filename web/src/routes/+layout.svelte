<script lang="ts">
    import '#lib/theme.css';
    import { goto } from '$app/navigation';
    import { page } from '$app/state';
    import { api } from '#lib/api.js';
    import { TAGLINE } from '#lib/brand.js';
    import type { MeResponse } from '#lib/types.js';
    import Badge from '#lib/components/Badge.svelte';

    let { children } = $props();

    // Theme: dark is the default; `light` forces light, `auto` follows the
    // OS preference. Persisted per browser; applied as data-theme on <html>
    // so lib/theme.css and the ECharts themes pick it up. Before the first
    // paint, static/early-theme.js puts the saved theme there: the layout
    // starts from it, and reads the storage itself only when the script did
    // not run, so that the saved choice is never lost.
    type Theme = 'dark' | 'light' | 'auto';
    const THEMES: Theme[] = ['dark', 'light', 'auto'];
    const THEME_GLYPH: Record<Theme, string> = { dark: '◐', light: '○', auto: '◑' };
    let theme = $state<Theme>(startTheme());
    function asTheme(value: string | null | undefined): Theme | undefined {
        return THEMES.find((t) => t === value);
    }
    function startTheme(): Theme {
        const set = asTheme(document.documentElement.dataset.theme);
        if (set) return set;
        try {
            return asTheme(localStorage.getItem('tumult.theme')) ?? 'dark';
        } catch {
            return 'dark'; // storage unavailable: stay dark
        }
    }
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
                            <Badge tone="neutral" label={me.role} />
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
        margin: var(--space-2) var(--space-4-5) 0;
        align-self: flex-start;
        background: none;
        border: var(--border-width-sm) solid var(--border);
        border-radius: var(--radius-md);
        color: var(--text-dim);
        font: inherit;
        font-size: var(--font-size-2xs);
        padding: var(--space-0-5) var(--space-2);
        cursor: pointer;
    }
    .theme:hover {
        color: var(--text);
        border-color: var(--border-strong);
    }
    .user-chip {
        margin-top: auto;
        padding: var(--space-3) var(--space-4-5) 0;
        border-top: var(--border-width-sm) solid var(--border);
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: var(--space-2);
    }
    .user-chip .who {
        display: flex;
        align-items: center;
        gap: var(--space-2);
        min-width: 0;
    }
    .user-chip .username {
        font-size: var(--font-size-md);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .user-chip .logout {
        background: transparent;
        border: var(--border-width-sm) solid var(--border-strong);
        color: var(--text-dim);
        border-radius: var(--radius-lg);
        padding: 3px var(--space-2-5);
        font-size: var(--font-size-xs);
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

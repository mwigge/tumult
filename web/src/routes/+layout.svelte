<script lang="ts">
  import '$lib/theme.css';
  import { page } from '$app/stores';

  const NAV = [
    { href: '/', label: 'Overview', icon: '◧' },
    { href: '/experiments', label: 'Experiments', icon: '⚗' },
    { href: '/logs', label: 'Logs', icon: '≣' },
    { href: '/ask', label: 'Ask', icon: '✦' },
    { href: '/reports', label: 'Reports', icon: '▤' }
  ];

  function active(pathname: string, href: string): boolean {
    return href === '/' ? pathname === '/' : pathname.startsWith(href);
  }
</script>

<div class="shell">
  <aside class="side">
    <div class="brand">
      <div class="name">kronika</div>
      <div class="tag">the chronicle of your resilience work</div>
    </div>
    <nav class="nav">
      {#each NAV as item (item.href)}
        <a href={item.href} class:active={active($page.url.pathname, item.href)}>
          <span>{item.icon}</span>{item.label}
        </a>
      {/each}
    </nav>
  </aside>
  <main class="main">
    <slot />
  </main>
</div>

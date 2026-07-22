<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { launchApp } from '$lib/apps/launcher.js';
  import type { Agent } from '$lib/api/nebo';
  import LayoutDashboard from 'lucide-svelte/icons/layout-dashboard';
  import PieChart from 'lucide-svelte/icons/pie-chart';
  import TrendingUp from 'lucide-svelte/icons/trending-up';
  import Users from 'lucide-svelte/icons/users';
  import BookOpen from 'lucide-svelte/icons/book-open';
  import Sparkles from 'lucide-svelte/icons/sparkles';
  import AppWindow from 'lucide-svelte/icons/app-window';
  import EllipsisVertical from 'lucide-svelte/icons/ellipsis-vertical';
  import { goto } from '$lib/nav';

  const COLOR_CYCLE = Object.keys(AGENT_COLORS_MAP);

  const ICON_MAP: Record<string, typeof LayoutDashboard> = {
    dashboard: LayoutDashboard,
    portfolio: PieChart,
    deal: TrendingUp,
    contact: Users,
    journal: BookOpen,
    hello: Sparkles,
  };

  function pickIcon(id: string): typeof LayoutDashboard {
    const lower = id.toLowerCase();
    for (const [key, icon] of Object.entries(ICON_MAP)) {
      if (lower.includes(key)) return icon;
    }
    return AppWindow;
  }

  type AppEntry = {
    id: string;
    name: string;
    icon: typeof LayoutDashboard;
    color: string;
    description: string;
  };

  const menuItems = [
    { id: 'open', label: 'apps.openApp' },
    { id: 'runs', label: 'apps.runs' },
    { id: 'settings', label: 'nav.settings' },
    { id: 'workflows', label: 'marketplace.workflows' },
    { id: 'persona', label: 'agent.persona' },
    { id: 'skills', label: 'commandPalette.skills' },
    { id: 'memory', label: 'apps.memory' },
    { id: 'permissions', label: 'commandPalette.permissions' },
  ];

  let apps = $state<AppEntry[]>([]);
  let openMenuId = $state<string | null>(null);


  function toggleMenu(e: MouseEvent, appId: string) {
    e.stopPropagation();
    openMenuId = openMenuId === appId ? null : appId;
  }

  function handleMenuItem(e: MouseEvent, appId: string, appName: string, itemId: string) {
    e.stopPropagation();
    openMenuId = null;
    if (itemId === 'open') {
      launchApp(appId, appName);
    } else {
      const section = itemId === 'settings' ? 'general' : itemId;
      goto(`/${appId}/settings/${section}`);
    }
  }

  function closeMenu() {
    openMenuId = null;
  }

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (!resp?.agents?.length) return;

      const agents = resp.agents as Agent[];
      const entries: AppEntry[] = [];
      for (const a of agents) {
        if (!a.isApp) continue;
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const displayName = (a as any).displayName as string || a.name;
        entries.push({
          id: a.id || a.name,
          name: displayName,
          icon: pickIcon(a.id || a.name),
          color: COLOR_CYCLE[entries.length % COLOR_CYCLE.length],
          description: a.description || '',
        });
      }
      entries.sort((a, b) => a.name.localeCompare(b.name));
      apps = entries;
    } catch { /* keep empty */ }
  });
</script>

<svelte:window onclick={closeMenu} />
<svelte:head><title>{$t('apps.pageTitle')}</title></svelte:head>

<div class="flex-1 flex flex-col bg-base-100 min-w-0">
  <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
    <span class="text-sm font-semibold">{$t('apps.title')}</span>
  </div>
  <div class="flex-1 overflow-y-auto p-6">
    {#if apps.length === 0}
      <div class="flex flex-col items-center justify-center py-16 gap-3">
        <div class="w-12 h-12 rounded-xl bg-base-200 flex items-center justify-center">
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></svg>
        </div>
        <div class="text-sm font-medium">{$t('settingsApps.noApps')}</div>
        <div class="text-xs text-base-content/50">{$t('apps.installHint')}</div>
      </div>
    {:else}
      <div class="grid grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
        {#each apps as app}
          {@const c = AGENT_COLORS_MAP[app.color as keyof typeof AGENT_COLORS_MAP]}
          <div
            class="p-5 rounded-lg border border-base-300 bg-base-200/50 cursor-pointer hover:border-primary/50 hover:shadow-sm transition-all text-left group relative"
            onclick={() => launchApp(app.id, app.name)}
            onkeydown={(e) => { if (e.key === 'Enter') launchApp(app.id, app.name); }}
            role="button"
            tabindex="0"
          >
            <div class="flex items-start justify-between mb-3">
              <div class="w-10 h-10 rounded-lg flex items-center justify-center {c.bgClass} {c.inkClass}"><app.icon class="w-5 h-5" /></div>
              <button
                class="w-7 h-7 rounded-md flex items-center justify-center opacity-0 group-hover:opacity-100 hover:bg-base-content/10 transition-all cursor-pointer"
                onclick={(e) => toggleMenu(e, app.id)}
              >
                <EllipsisVertical class="w-4 h-4 text-base-content/50" />
              </button>
            </div>
            <div class="text-sm font-medium mb-1">{app.name}</div>
            <div class="text-xs text-base-content/70 line-clamp-2">{app.description}</div>
            <div class="mt-3 text-xs text-primary font-medium opacity-0 group-hover:opacity-100 transition-opacity">{$t('apps.openAppHint')}</div>

            {#if openMenuId === app.id}
              <div class="absolute top-12 right-3 z-50 w-44 py-1 rounded-lg border border-base-300 bg-base-100 shadow-lg">
                {#each menuItems as item}
                  {#if item.id === 'runs'}
                    <div class="h-px bg-base-content/10 my-1"></div>
                  {/if}
                  <button
                    class="w-full text-left px-3 py-1.5 text-sm hover:bg-base-200/50 transition-colors cursor-pointer {item.id === 'open' ? 'font-medium' : ''}"
                    onclick={(e) => handleMenuItem(e, app.id, app.name, item.id)}
                  >{$t(item.label)}</button>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>


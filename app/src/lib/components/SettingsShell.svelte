<script lang="ts">
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { devMode } from '$lib/stores/devmode.js';
  import Cloud from 'lucide-svelte/icons/cloud';
  import User from 'lucide-svelte/icons/user';
  import CreditCard from 'lucide-svelte/icons/credit-card';
  import Bot from 'lucide-svelte/icons/bot';
  import Zap from 'lucide-svelte/icons/zap';
  import Puzzle from 'lucide-svelte/icons/puzzle';
  import Key from 'lucide-svelte/icons/key';
  import Cpu from 'lucide-svelte/icons/cpu';
  import Lock from 'lucide-svelte/icons/lock';
  import Cable from 'lucide-svelte/icons/cable';
  import Globe from 'lucide-svelte/icons/globe';
  import Shield from 'lucide-svelte/icons/shield';
  import Code from 'lucide-svelte/icons/code';
  import BarChart3 from 'lucide-svelte/icons/bar-chart-3';
  import Activity from 'lucide-svelte/icons/activity';
  import Info from 'lucide-svelte/icons/info';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import Radio from 'lucide-svelte/icons/radio';
  import X from 'lucide-svelte/icons/x';
  import type { SvelteComponent } from 'svelte';

  import { onMount } from 'svelte';

  let { children } = $props();

  let appVersion = $state('');

  onMount(async () => {
    try {
      const resp = await fetch('/health');
      const data = await resp.json();
      if (data?.version) appVersion = data.version;
    } catch { /* keep empty */ }
  });

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  interface NavItem { id: string; path: string; label: string; icon: typeof SvelteComponent<any>; devOnly?: boolean }

  const allItems: (NavItem | null)[] = [
    { id: 'account', path: '/settings/account', label: 'settings.navItems.account', icon: Cloud },
    { id: 'profile', path: '/settings/profile', label: 'settings.navItems.profile', icon: User },
    { id: 'billing', path: '/settings/billing', label: 'settings.navItems.billing', icon: CreditCard },
    { id: 'usage', path: '/settings/usage', label: 'settings.navItems.usage', icon: BarChart3 },
    null,
    { id: 'agents', path: '/settings/agents', label: 'settings.navItems.agents', icon: Bot },
    { id: 'skills', path: '/settings/skills', label: 'settings.navItems.skills', icon: Zap },
    { id: 'plugins', path: '/settings/plugins', label: 'settings.navItems.plugins', icon: Puzzle },
    { id: 'mcp', path: '/settings/mcp', label: 'settings.navItems.mcp', icon: Cable },
    { id: 'browser', path: '/settings/browser', label: 'settings.navItems.browser', icon: Globe },
    { id: 'updates', path: '/settings/updates', label: 'settings.navItems.updates', icon: RefreshCw },
    null,
    { id: 'providers', path: '/settings/providers', label: 'settings.navItems.providers', icon: Key, devOnly: true },
    { id: 'routing', path: '/settings/routing', label: 'settings.navItems.routing', icon: Cpu, devOnly: true },
    { id: 'secrets', path: '/settings/secrets', label: 'settings.navItems.secrets', icon: Lock, devOnly: true },
    { id: 'events', path: '/settings/events', label: 'settings.navItems.systemEvents', icon: Radio, devOnly: true },
    null,
    { id: 'permissions', path: '/settings/permissions', label: 'settings.navItems.permissions', icon: Shield },
    null,
    { id: 'status', path: '/settings/status', label: 'settings.navItems.status', icon: Activity },
    null,
    { id: 'developer', path: '/settings/developer', label: 'settings.navItems.developer', icon: Code },
    null,
    { id: 'about', path: '/settings/about', label: 'settings.navItems.about', icon: Info },
  ];

  // Filter out devOnly items when dev mode is off, and collapse adjacent nulls
  const items = $derived.by(() => {
    const filtered: (NavItem | null)[] = [];
    for (const item of allItems) {
      if (item !== null && item.devOnly && !$devMode) continue;
      // Skip null if previous item was also null (avoid double gaps)
      if (item === null && filtered.length > 0 && filtered[filtered.length - 1] === null) continue;
      filtered.push(item);
    }
    // Trim trailing null
    if (filtered.length > 0 && filtered[filtered.length - 1] === null) filtered.pop();
    return filtered;
  });

  const allTabs = $derived(items.filter((i): i is NavItem => i !== null));
  const activeTab = $derived(
    allTabs.find(t => $page.url.pathname.startsWith(t.path))?.id || 'account'
  );

  function closeSettings() {
    goto('/');
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') closeSettings();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="fixed inset-0 z-60 flex items-center justify-center p-4 sm:p-8">
  <!-- Backdrop -->
  <div class="absolute inset-0 bg-black/60 backdrop-blur-sm"></div>

  <!-- Modal card -->
  <div class="relative w-full max-w-4xl flex flex-col rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden" style="height: calc(100vh - 4rem)">
    <!-- Header -->
    <div class="flex items-center justify-between px-6 py-4 border-b border-base-content/10 shrink-0">
      <div class="flex items-center gap-3">
        <h1 class="font-display text-lg font-bold text-base-content">{$t('settings.title')}</h1>
        {#if appVersion}<span class="text-xs text-base-content/50">v{appVersion}</span>{/if}
      </div>
      <button
        class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors cursor-pointer bg-transparent border-none"
        onclick={closeSettings}
        aria-label={$t('settings.closeSettings')}
      >
        <X class="w-4 h-4 text-base-content/90" />
      </button>
    </div>

    <!-- Body: sidebar + content -->
    <div class="flex flex-1 min-h-0 overflow-hidden">
      <!-- Nav sidebar -->
      <nav class="w-48 shrink-0 border-r border-base-content/10 overflow-y-auto py-3 px-2" aria-label={$t('settings.settingsNav')}>
        <ul class="flex flex-col gap-0.5">
          {#each items as item}
            {#if item === null}
              <li class="h-3"></li>
            {:else}
              <li>
                <a
                  href={item.path}
                  class="w-full flex items-center gap-2.5 px-3 py-1.5 rounded-lg text-sm text-left transition-colors whitespace-nowrap {activeTab === item.id
                    ? 'bg-primary/10 text-primary ring-1 ring-primary/20'
                    : 'text-base-content/90 hover:bg-base-200 hover:text-base-content'}"
                  aria-current={activeTab === item.id ? 'page' : undefined}
                >
                  <item.icon class="w-4 h-4 shrink-0" />
                  <span class="font-medium">{$t(item.label)}</span>
                </a>
              </li>
            {/if}
          {/each}
        </ul>
      </nav>

      <!-- Content -->
      <main class="flex-1 min-w-0 overflow-y-auto p-6">
        <div class="max-w-2xl">
          {@render children()}
        </div>
      </main>
    </div>
  </div>
</div>

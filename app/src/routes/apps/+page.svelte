<script lang="ts">
  import { onMount } from 'svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { launchApp } from '$lib/apps/launcher.js';
  import type { Agent } from '$lib/api/nebo';

  const COLOR_CYCLE = Object.keys(AGENT_COLORS_MAP);

  type AppEntry = {
    id: string;
    name: string;
    initial: string;
    color: string;
    description: string;
  };

  let apps = $state<AppEntry[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (!resp?.agents?.length) return;

      const entries: AppEntry[] = [];
      for (let i = 0; i < resp.agents.length; i++) {
        const a = resp.agents[i] as Agent & { isApp?: boolean };
        if (!(a as any).isApp) continue;
        entries.push({
          id: a.id,
          name: a.name,
          initial: a.name.charAt(0).toUpperCase(),
          color: COLOR_CYCLE[i % COLOR_CYCLE.length],
          description: a.description || '',
        });
      }
      apps = entries;
    } catch { /* keep empty */ }
  });
</script>

<svelte:head><title>Apps - Nebo</title></svelte:head>

<div class="flex-1 flex flex-col bg-base-100 min-w-0">
  <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
    <span class="text-sm font-semibold">Apps</span>
  </div>
  <div class="flex-1 overflow-y-auto p-6">
    {#if apps.length === 0}
      <div class="flex flex-col items-center justify-center py-16 gap-3">
        <div class="w-12 h-12 rounded-xl bg-base-200 flex items-center justify-center">
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/></svg>
        </div>
        <div class="text-sm font-medium">No apps installed</div>
        <div class="text-xs text-base-content/50">Install apps from the marketplace using an APPX- code</div>
      </div>
    {:else}
      <div class="grid grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4">
        {#each apps as app}
          {@const c = AGENT_COLORS_MAP[app.color as keyof typeof AGENT_COLORS_MAP]}
          <button
            class="p-5 rounded-lg border border-base-300 bg-base-200/50 cursor-pointer hover:border-primary/50 hover:shadow-sm transition-all text-left group"
            onclick={() => launchApp(app.id, app.name)}
          >
            <div class="w-10 h-10 rounded-lg flex items-center justify-center text-base font-mono font-semibold mb-3 {c.bgClass} {c.inkClass}">{app.initial}</div>
            <div class="text-sm font-medium mb-1">{app.name}</div>
            <div class="text-xs text-base-content/70 line-clamp-2">{app.description}</div>
            <div class="mt-3 text-xs text-primary font-medium opacity-0 group-hover:opacity-100 transition-opacity">Open app</div>
          </button>
        {/each}
      </div>
    {/if}
  </div>
</div>

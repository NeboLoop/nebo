<script lang="ts">
  import { onMount } from 'svelte';
  import { installedItems, uninstallItem, loadInstalledItems } from '$lib/stores/marketplace.js';
  import Package from 'lucide-svelte/icons/package';
  import Star from 'lucide-svelte/icons/star';

  onMount(() => { loadInstalledItems(); });

  const iconColors = [
    'bg-primary/15 text-primary', 'bg-accent/15 text-accent', 'bg-success/15 text-success',
    'bg-warning/15 text-warning', 'bg-error/15 text-error', 'bg-info/15 text-info', 'bg-secondary/15 text-secondary',
  ];
  function getIconColor(id: string) {
    let hash = 0;
    for (let i = 0; i < id.length; i++) hash = id.charCodeAt(i) + ((hash << 5) - hash);
    return iconColors[Math.abs(hash) % iconColors.length];
  }
  function getInitials(name: string) {
    return name.split(' ').map(w => w[0]).join('').slice(0, 2).toUpperCase();
  }

  const typeLabels: Record<string, string> = { skill: 'Skill', agent: 'Agent', plugin: 'Plugin', connector: 'MCP' };

  function viewItem(item: { id: string; type: string }) {
    window.location.href = `/marketplace/${item.type}s/${item.id}`;
  }
</script>

<svelte:head><title>Installed - Marketplace - Nebo</title></svelte:head>

<div class="p-6 max-w-[960px]">
  <div class="mb-5">
    <div class="text-base font-semibold mb-1">Installed</div>
    <div class="text-xs text-base-content/50">Manage your installed skills, agents, plugins, and connectors.</div>
  </div>

  {#if $installedItems.length === 0}
    <div class="text-center py-12">
      <Package class="w-8 h-8 text-base-content/30 mx-auto mb-3" />
      <div class="text-sm font-medium mb-1">No items installed yet</div>
      <a href="/marketplace" class="text-xs text-primary hover:underline">Browse marketplace &rarr;</a>
    </div>
  {:else}
    <div class="flex flex-col gap-2">
      {#each $installedItems as item}
        <div class="flex items-center gap-3 py-3 px-4 rounded-xl border border-base-300 bg-base-100">
          <div class="w-9 h-9 rounded-lg {getIconColor(item.id)} grid place-items-center text-xs font-bold shrink-0">{getInitials(item.name)}</div>
          <div class="flex-1 min-w-0">
            <div class="text-sm font-medium mb-0.5">{item.name}</div>
            <div class="text-xs text-base-content/50">Installed {item.installed} · {typeLabels[item.type] ?? item.type}</div>
          </div>
          <div class="flex gap-1.5">
            <button class="px-3 py-1 rounded-lg border border-base-300 text-xs font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors" onclick={() => viewItem(item)}>View</button>
            <button class="px-3 py-1 rounded-lg border border-error/20 text-xs font-medium text-error cursor-pointer bg-transparent hover:bg-error/5 transition-colors" onclick={() => uninstallItem(item.id)}>Uninstall</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<script lang="ts">
  import { onMount } from 'svelte';
  import { listStoreProducts } from '$lib/api/index';
  import { installedIds } from '$lib/stores/marketplace.js';
  import Star from 'lucide-svelte/icons/star';

  const iconColors = [
    'bg-primary/15 text-primary',
    'bg-accent/15 text-accent',
    'bg-success/15 text-success',
    'bg-warning/15 text-warning',
    'bg-error/15 text-error',
    'bg-info/15 text-info',
    'bg-secondary/15 text-secondary',
  ];
  function getIconColor(id: string) {
    let hash = 0;
    for (let i = 0; i < id.length; i++) hash = id.charCodeAt(i) + ((hash << 5) - hash);
    return iconColors[Math.abs(hash) % iconColors.length];
  }
  function getInitials(name: string) {
    return name.split(' ').map(w => w[0]).join('').slice(0, 2).toUpperCase();
  }

  let plugins = $state<{ id: string; name: string; desc: string; category: string; rating: number; installs: number; featured: boolean; price: string; code: string }[]>([]);

  onMount(async () => {
    try {
      const res = await listStoreProducts({ type: 'plugin' });
      if (res?.apps?.length) {
        plugins = res.apps.map((a: Record<string, unknown>) => ({
          id: a.id, name: a.name, desc: a.description || '',
          category: a.category || '', rating: a.rating || 0,
          installs: a.installCount || 0, featured: a.featured ?? false,
          price: a.price || 'Get', code: a.code || '',
        }));
      }
    } catch {}
  });
</script>

<svelte:head><title>Plugins - Marketplace - Nebo</title></svelte:head>

<div class="p-6 max-w-[960px]">
  <div class="mb-5">
    <div class="text-base font-semibold mb-1">Plugins</div>
    <div class="text-xs text-base-content/50">Connect your agents to external services and APIs.</div>
  </div>

  <div class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
    {#each plugins as plugin}
      <a href="/marketplace/plugins/{plugin.id}" class="p-4 rounded-xl border border-base-300 bg-base-100 cursor-pointer hover:shadow-md hover:border-base-content/20 transition-all block group">
        <div class="w-11 h-11 rounded-xl {getIconColor(plugin.id)} grid place-items-center text-sm font-bold mb-3">
          {getInitials(plugin.name)}
        </div>
        <div class="text-sm font-semibold mb-0.5 group-hover:text-primary transition-colors">{plugin.name}</div>
        <div class="text-xs text-base-content/60 leading-snug mb-3 line-clamp-2">{plugin.desc}</div>
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-1.5">
            <Star class="w-3 h-3 text-warning fill-warning" />
            <span class="text-xs font-medium">{plugin.rating}</span>
            <span class="text-xs text-base-content/40 ml-0.5">{plugin.installs.toLocaleString()}</span>
          </div>
          {#if $installedIds.has(plugin.id)}
            <span class="text-xs font-medium text-success">Installed</span>
          {:else if plugin.price === 'Get'}
            <span class="text-xs font-medium text-primary">Free</span>
          {:else}
            <span class="text-xs text-base-content/50">{plugin.price}</span>
          {/if}
        </div>
      </a>
    {/each}
  </div>
</div>

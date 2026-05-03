<script lang="ts">
  import { onMount } from 'svelte';
  import { listStoreProducts } from '$lib/api/index';
  import { installedIds } from '$lib/stores/marketplace.js';
  import Star from 'lucide-svelte/icons/star';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';

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

  type MItem = { id: string; name: string; desc: string; category: string; rating: number; installs: number; type: string; path: string; price: string };
  let allItems = $state<MItem[]>([]);
  let apiCategories = $state<{ slug: string; name: string; emoji: string; count: number }[]>([]);

  onMount(async () => {
    try {
      const res = await listStoreProducts();
      if (res?.apps?.length) {
        const typeMap: Record<string, string> = { agent: 'agents', skill: 'skills', plugin: 'plugins', connector: 'connectors' };
        allItems = res.apps.map((a: Record<string, unknown>) => {
          const t = a.type || 'skill';
          return {
            id: a.id, name: a.name, desc: a.description || '',
            category: a.category || '', rating: a.rating || 0,
            installs: a.installCount || 0, price: a.price || 'Get',
            type: t, path: `/marketplace/${typeMap[t] || 'skills'}/${a.id}`,
          };
        });
        const catMap = new Map<string, number>();
        for (const p of allItems) {
          if (p.category) catMap.set(p.category, (catMap.get(p.category) || 0) + 1);
        }
        apiCategories = [...catMap.entries()].map(([slug, count]) => ({
          slug, name: slug.charAt(0).toUpperCase() + slug.slice(1), emoji: '', count,
        }));
      }
    } catch {}
  });

  const typeLabels: Record<string, string> = { skill: 'Skill', agent: 'Agent', plugin: 'Plugin', connector: 'MCP' };

  // Group items by category
  function getItemsForCategory(slug: string) {
    return allItems.filter(i => i.category === slug).sort((a, b) => b.rating - a.rating).slice(0, 4);
  }
</script>

<svelte:head><title>Categories - Marketplace - Nebo</title></svelte:head>

<div class="p-6 max-w-[960px]">
  <div class="mb-6">
    <div class="text-base font-semibold mb-1">Categories</div>
    <div class="text-xs text-base-content/50">Browse skills, agents, and plugins by category.</div>
  </div>

  <div class="flex flex-col gap-8">
    {#each apiCategories as cat}
      {@const items = getItemsForCategory(cat.slug)}
      {#if items.length > 0}
        <div>
          <div class="flex items-center justify-between mb-3">
            <div>
              <div class="text-sm font-semibold">{cat.name}</div>
              <div class="text-xs text-base-content/50">{cat.count} items</div>
            </div>
          </div>
          <div class="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3">
            {#each items as item}
              <a href={item.path} class="p-4 rounded-xl border border-base-300 bg-base-100 hover:shadow-md hover:border-base-content/20 transition-all group block">
                <div class="w-10 h-10 rounded-xl {getIconColor(item.id)} grid place-items-center text-sm font-bold mb-3">
                  {getInitials(item.name)}
                </div>
                <div class="text-sm font-semibold mb-0.5 group-hover:text-primary transition-colors">{item.name}</div>
                <div class="text-xs text-base-content/60 leading-snug mb-2.5 line-clamp-2">{item.desc}</div>
                <div class="flex items-center justify-between">
                  <div class="flex items-center gap-1">
                    <Star class="w-3 h-3 text-warning fill-warning" />
                    <span class="text-xs font-medium">{item.rating}</span>
                    <span class="text-xs text-base-content/40 ml-1">{item.installs.toLocaleString()}</span>
                  </div>
                  <span class="py-0.5 px-1.5 rounded-full bg-base-200 text-xs text-base-content/60">{typeLabels[item.type]}</span>
                </div>
              </a>
            {/each}
          </div>
        </div>
      {/if}
    {/each}
  </div>
</div>

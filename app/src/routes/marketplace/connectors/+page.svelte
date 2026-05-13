<script lang="ts">
  import { onMount } from 'svelte';
  import { listStoreProducts } from '$lib/api/index';
  import { installedIds } from '$lib/stores/marketplace.js';
  import Star from 'lucide-svelte/icons/star';

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

  let connectors = $state<{ id: string; name: string; desc: string; category: string; rating: number; installs: number; featured: boolean; price: string; code: string; authType: string }[]>([]);

  onMount(async () => {
    try {
      const res = await listStoreProducts({ type: 'connector' }) as { apps?: Record<string, unknown>[] } | null;
      if (res?.apps?.length) {
        connectors = res.apps.map((a: Record<string, unknown>) => ({
          id: String(a.id ?? ''), name: String(a.name ?? ''), desc: String(a.description ?? ''),
          category: String(a.category ?? ''), rating: Number(a.rating ?? 0),
          installs: Number(a.installCount ?? 0), featured: Boolean(a.featured ?? false),
          price: String(a.price ?? 'Get'), code: String(a.code ?? ''),
          authType: String(a.authType ?? 'none'),
        }));
      }
    } catch {}
  });
</script>

<svelte:head><title>Connectors - Marketplace - Nebo</title></svelte:head>

<div class="p-6 max-w-[960px]">
  <div class="mb-5">
    <div class="text-base font-semibold mb-1">Connectors</div>
    <div class="text-xs text-base-content/50">MCP servers that give your agents access to tools, databases, and APIs.</div>
  </div>

  <div class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
    {#each connectors as connector}
      <a href="/marketplace/connectors/{connector.id}" class="p-4 rounded-xl border border-base-300 bg-base-100 cursor-pointer hover:shadow-md hover:border-base-content/20 transition-all block group">
        <div class="w-11 h-11 rounded-xl {getIconColor(connector.id)} grid place-items-center text-sm font-bold mb-3">
          {getInitials(connector.name)}
        </div>
        <div class="text-sm font-semibold mb-0.5 group-hover:text-primary transition-colors">{connector.name}</div>
        <div class="text-xs text-base-content/60 leading-snug mb-3 line-clamp-2">{connector.desc}</div>
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-1.5">
            <Star class="w-3 h-3 text-warning fill-warning" />
            <span class="text-xs font-medium">{connector.rating}</span>
            <span class="text-xs text-base-content/40 ml-0.5">{connector.installs.toLocaleString()}</span>
          </div>
          {#if $installedIds.has(connector.id)}
            <span class="text-xs font-medium text-success">Installed</span>
          {:else}
            <span class="text-xs font-medium text-primary">Free</span>
          {/if}
        </div>
      </a>
    {/each}
  </div>
</div>

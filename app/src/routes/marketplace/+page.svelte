<script lang="ts">
  import { onMount } from 'svelte';
  import { listStoreProducts } from '$lib/api/index';
  import { installedIds, loadInstalledItems } from '$lib/stores/marketplace.js';
  import Star from 'lucide-svelte/icons/star';
  import Download from 'lucide-svelte/icons/download';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';
  import Sparkles from 'lucide-svelte/icons/sparkles';
  import Zap from 'lucide-svelte/icons/zap';

  type MItem = { id: string; name: string; desc: string; category: string; rating: number; installs: number; featured: boolean; price: string; code: string; type: string; path: string };
  let allItems = $state<MItem[]>([]);
  let apiCategories = $state<{ slug: string; name: string; emoji: string; count: number }[]>([]);
  let apiOrgs = $state<Record<string, unknown>[]>([]);
  let apiPrivateItems = $state<Record<string, unknown>[]>([]);

  onMount(async () => {
    loadInstalledItems();
    try {
      const res = await listStoreProducts();
      if (res?.apps?.length) {
        const typeMap: Record<string, string> = { agent: 'agents', skill: 'skills', plugin: 'plugins', connector: 'connectors' };
        allItems = res.apps.map((a: Record<string, unknown>) => {
          const t = a.type || 'skill';
          return {
            id: a.id, name: a.name, desc: a.description || '',
            category: a.category || '', rating: a.rating || 0,
            installs: a.installCount || 0, featured: a.featured ?? false,
            price: a.price || 'Get', code: a.code || '',
            type: t, path: `/marketplace/${typeMap[t] || 'skills'}/${a.id}`,
          };
        });
        // Derive categories
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

  // Color palette for item icons (derived from item id hash)
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

  // Hero featured items — top rated across all types
  const heroItems = $derived(
    allItems.filter(i => i.featured).sort((a, b) => b.rating - a.rating).slice(0, 3)
  );

  // Staff picks — highest install counts
  const staffPicks = $derived(
    allItems.sort((a, b) => b.installs - a.installs).slice(0, 6)
  );

  // Per-type subsets
  const agentItems = $derived(allItems.filter(i => i.type === 'agent').slice(0, 6));
  const skillItems = $derived(allItems.filter(i => i.type === 'skill').slice(0, 8));
  const pluginItems = $derived(allItems.filter(i => i.type === 'plugin').slice(0, 8));

  const typeLabels: Record<string, string> = { skill: 'Skill', agent: 'Agent', plugin: 'Plugin', connector: 'MCP' };
</script>

<div class="p-6 max-w-[960px]">
  <!-- Hero Banner -->
  <div class="rounded-2xl bg-primary/8 border border-primary/15 p-6 mb-8">
    <div class="flex items-center gap-2 mb-1">
      <Sparkles class="w-4 h-4 text-primary" />
      <span class="text-xs font-semibold uppercase tracking-wider text-primary">Featured</span>
    </div>
    <h1 class="text-lg font-semibold mb-1">Supercharge your workflow</h1>
    <p class="text-xs text-base-content/60 mb-5">Hand-picked skills, agents, and plugins to get the most out of Nebo.</p>

    <div class="grid grid-cols-3 gap-3">
      {#each heroItems as item}
        <a href={item.path} class="flex items-start gap-3 p-3.5 rounded-xl bg-base-100 border border-base-300 hover:shadow-md hover:border-base-content/20 transition-all group">
          <div class="w-10 h-10 rounded-xl {getIconColor(item.id)} grid place-items-center text-sm font-bold shrink-0">
            {getInitials(item.name)}
          </div>
          <div class="flex-1 min-w-0">
            <div class="text-sm font-semibold truncate group-hover:text-primary transition-colors">{item.name}</div>
            <div class="text-xs text-base-content/60 leading-snug line-clamp-2 mb-1.5">{item.desc}</div>
            <div class="flex items-center gap-2">
              <div class="flex items-center gap-0.5">
                <Star class="w-3 h-3 text-warning fill-warning" />
                <span class="text-xs font-medium">{item.rating}</span>
              </div>
              <span class="text-xs text-base-content/40">·</span>
              <span class="text-xs text-base-content/50">{item.installs.toLocaleString()}</span>
              <span class="text-xs text-base-content/40">·</span>
              <span class="py-0.5 px-1.5 rounded-full bg-base-200 text-xs text-base-content/60">{typeLabels[item.type]}</span>
            </div>
          </div>
        </a>
      {/each}
    </div>
  </div>

  <!-- Staff Picks -->
  <div class="mb-8">
    <div class="flex items-center justify-between mb-4">
      <div>
        <div class="text-sm font-semibold">Popular right now</div>
        <div class="text-xs text-base-content/50">Most installed across the marketplace</div>
      </div>
    </div>
    <div class="grid grid-cols-2 gap-3">
      {#each staffPicks as item}
        <a href={item.path} class="flex items-center gap-3.5 p-3.5 rounded-xl border border-base-300 bg-base-100 hover:shadow-md hover:border-base-content/20 transition-all group">
          <div class="w-11 h-11 rounded-xl {getIconColor(item.id)} grid place-items-center text-sm font-bold shrink-0">
            {getInitials(item.name)}
          </div>
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2 mb-0.5">
              <span class="text-sm font-semibold truncate group-hover:text-primary transition-colors">{item.name}</span>
              {#if $installedIds.has(item.id)}
                <span class="text-xs font-medium text-success shrink-0">Installed</span>
              {:else if item.price === 'Get'}
                <span class="text-xs font-medium text-primary shrink-0">Free</span>
              {:else}
                <span class="text-xs text-base-content/50 shrink-0">{item.price}</span>
              {/if}
            </div>
            <div class="text-xs text-base-content/60 truncate mb-1">{item.desc}</div>
            <div class="flex items-center gap-2">
              <div class="flex items-center gap-0.5">
                <Star class="w-3 h-3 text-warning fill-warning" />
                <span class="text-xs font-medium">{item.rating}</span>
              </div>
              <span class="text-xs text-base-content/40">·</span>
              <span class="text-xs text-base-content/50">{item.installs.toLocaleString()} installs</span>
              <span class="text-xs text-base-content/40">·</span>
              <span class="py-0.5 px-1.5 rounded-full bg-base-200 text-xs text-base-content/60">{typeLabels[item.type]}</span>
            </div>
          </div>
        </a>
      {/each}
    </div>
  </div>

  <!-- Top Agents -->
  <div class="mb-8">
    <div class="flex items-center justify-between mb-4">
      <div>
        <div class="text-sm font-semibold">Top Agents</div>
        <div class="text-xs text-base-content/50">Autonomous agents that handle complex workflows</div>
      </div>
      <a href="/marketplace/agents" class="flex items-center gap-1 text-xs text-primary font-medium hover:underline">
        View all <ArrowRight class="w-3 h-3" />
      </a>
    </div>
    <div class="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3">
      {#each agentItems as agent}
        <a href="/marketplace/agents/{agent.id}" class="p-4 rounded-xl border border-base-300 bg-base-100 hover:shadow-md hover:border-base-content/20 transition-all group block">
          <div class="w-10 h-10 rounded-xl {getIconColor(agent.id)} grid place-items-center text-sm font-bold mb-3">
            {getInitials(agent.name)}
          </div>
          <div class="text-sm font-semibold mb-0.5 group-hover:text-primary transition-colors">{agent.name}</div>
          <div class="text-xs text-base-content/60 leading-snug mb-2.5 line-clamp-2">{agent.desc}</div>
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-1">
              <Star class="w-3 h-3 text-warning fill-warning" />
              <span class="text-xs font-medium">{agent.rating}</span>
              <span class="text-xs text-base-content/40 ml-1">{agent.installs.toLocaleString()}</span>
            </div>
            {#if $installedIds.has(agent.id)}
              <span class="text-xs font-medium text-success">Installed</span>
            {:else if agent.price === 'Get'}
              <span class="text-xs font-medium text-primary">Free</span>
            {:else}
              <span class="text-xs text-base-content/50">{agent.price}</span>
            {/if}
          </div>
        </a>
      {/each}
    </div>
  </div>

  <!-- Skills -->
  <div class="mb-8">
    <div class="flex items-center justify-between mb-4">
      <div>
        <div class="text-sm font-semibold">Essential Skills</div>
        <div class="text-xs text-base-content/50">Capabilities you can add to any agent</div>
      </div>
      <a href="/marketplace/skills" class="flex items-center gap-1 text-xs text-primary font-medium hover:underline">
        View all <ArrowRight class="w-3 h-3" />
      </a>
    </div>
    <div class="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3">
      {#each skillItems as skill}
        <a href="/marketplace/skills/{skill.id}" class="p-4 rounded-xl border border-base-300 bg-base-100 hover:shadow-md hover:border-base-content/20 transition-all group block">
          <div class="w-10 h-10 rounded-xl {getIconColor(skill.id)} grid place-items-center text-sm font-bold mb-3">
            {getInitials(skill.name)}
          </div>
          <div class="text-sm font-semibold mb-0.5 group-hover:text-primary transition-colors">{skill.name}</div>
          <div class="text-xs text-base-content/60 leading-snug mb-2.5 line-clamp-2">{skill.desc}</div>
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-1">
              <Star class="w-3 h-3 text-warning fill-warning" />
              <span class="text-xs font-medium">{skill.rating}</span>
              <span class="text-xs text-base-content/40 ml-1">{skill.installs.toLocaleString()}</span>
            </div>
            {#if $installedIds.has(skill.id)}
              <span class="text-xs font-medium text-success">Installed</span>
            {:else if skill.price === 'Get'}
              <span class="text-xs font-medium text-primary">Free</span>
            {:else}
              <span class="text-xs text-base-content/50">{skill.price}</span>
            {/if}
          </div>
        </a>
      {/each}
    </div>
  </div>

  <!-- Plugins -->
  <div class="mb-8">
    <div class="flex items-center justify-between mb-4">
      <div>
        <div class="text-sm font-semibold">Connect your tools</div>
        <div class="text-xs text-base-content/50">Integrations with the apps you already use</div>
      </div>
      <a href="/marketplace/plugins" class="flex items-center gap-1 text-xs text-primary font-medium hover:underline">
        View all <ArrowRight class="w-3 h-3" />
      </a>
    </div>
    <div class="grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-3">
      {#each pluginItems as plugin}
        <a href="/marketplace/plugins/{plugin.id}" class="p-4 rounded-xl border border-base-300 bg-base-100 hover:shadow-md hover:border-base-content/20 transition-all group block">
          <div class="w-10 h-10 rounded-xl {getIconColor(plugin.id)} grid place-items-center text-sm font-bold mb-3">
            {getInitials(plugin.name)}
          </div>
          <div class="text-sm font-semibold mb-0.5 group-hover:text-primary transition-colors">{plugin.name}</div>
          <div class="text-xs text-base-content/60 leading-snug mb-2.5 line-clamp-2">{plugin.desc}</div>
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-1">
              <Star class="w-3 h-3 text-warning fill-warning" />
              <span class="text-xs font-medium">{plugin.rating}</span>
              <span class="text-xs text-base-content/40 ml-1">{plugin.installs.toLocaleString()}</span>
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

  <!-- Browse by Category -->
  <div class="mb-8">
    <div class="flex items-center justify-between mb-4">
      <div>
        <div class="text-sm font-semibold">Browse by category</div>
        <div class="text-xs text-base-content/50">Find exactly what you need</div>
      </div>
      <a href="/marketplace/categories" class="flex items-center gap-1 text-xs text-primary font-medium hover:underline">
        All categories <ArrowRight class="w-3 h-3" />
      </a>
    </div>
    <div class="grid grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-2.5">
      {#each apiCategories.slice(0, 12) as cat}
        <a href="/marketplace/categories" class="flex items-center gap-2.5 py-2.5 px-3.5 rounded-xl border border-base-300 bg-base-100 hover:shadow-md hover:border-base-content/20 transition-all group">
          <span class="text-sm font-medium group-hover:text-primary transition-colors">{cat.name}</span>
        </a>
      {/each}
    </div>
  </div>

  <!-- Collections -->
  {#if apiOrgs.length > 0}
    <div class="mb-8">
      <div class="flex items-center justify-between mb-4">
        <div>
          <div class="text-sm font-semibold">Collections</div>
          <div class="text-xs text-base-content/50">Curated bundles shared with you</div>
        </div>
        <a href="/marketplace/collections" class="flex items-center gap-1 text-xs text-primary font-medium hover:underline">
          View all <ArrowRight class="w-3 h-3" />
        </a>
      </div>
      <div class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
        {#each apiOrgs as org}
          {@const orgItems = apiPrivateItems.filter(i => i.orgId === org.id)}
          <a href="/marketplace/collections/{org.id}" class="p-4 rounded-xl border border-base-300 bg-base-100 hover:shadow-md hover:border-base-content/20 transition-all group block">
            <div class="flex items-center gap-2.5 mb-2.5">
              <div class="w-10 h-10 rounded-xl bg-base-200 grid place-items-center text-sm font-bold shrink-0">{org.initial}</div>
              <div class="flex-1 min-w-0">
                <div class="text-sm font-semibold truncate group-hover:text-primary transition-colors">{org.name}</div>
                <div class="text-xs text-base-content/50">{org.itemCount} items</div>
              </div>
            </div>
            <div class="text-xs text-base-content/60 leading-snug">
              {orgItems.slice(0, 2).map(i => i.name).join(', ')}{orgItems.length > 2 ? `, +${orgItems.length - 2} more` : ''}
            </div>
          </a>
        {/each}
      </div>
    </div>
  {/if}

  <!-- Build CTA -->
  <div class="p-6 rounded-2xl border border-primary/15 bg-primary/5 text-center">
    <div class="flex items-center justify-center gap-2 mb-1">
      <Zap class="w-4 h-4 text-primary" />
      <span class="text-sm font-semibold">Build for Nebo</span>
    </div>
    <div class="text-xs text-base-content/60 mb-3 max-w-[320px] mx-auto">Create and publish your own skills, agents, and plugins to the marketplace.</div>
    <button class="py-2 px-5 rounded-lg bg-primary text-primary-content text-sm font-medium cursor-pointer border-none hover:brightness-110 transition-all">
      Get Started <span class="ml-1">&rarr;</span>
    </button>
  </div>
</div>

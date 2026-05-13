<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { getStoreProduct, getStoreProductReviews } from '$lib/api/index';
  import { installedIds, installItem, uninstallItem } from '$lib/stores/marketplace.js';
  import OAuthConnectModal from '$lib/components/OAuthConnectModal.svelte';
  import Star from 'lucide-svelte/icons/star';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import ShieldCheck from 'lucide-svelte/icons/shield-check';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import Monitor from 'lucide-svelte/icons/monitor';
  import Globe from 'lucide-svelte/icons/globe';
  import Mail from 'lucide-svelte/icons/mail';
  import Calendar from 'lucide-svelte/icons/calendar';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';

  interface Screenshot { title: string; desc: string }
  interface Developer { name: string; website: string; support: string; launched: string }
  interface UsedByAgent { id: string; name: string; category: string }
  interface ReviewItem { user: string; rating: number; text: string; date: string; role: string; duration: string }

  interface PluginDetail {
    id: string; name: string; desc: string; category: string; rating: number;
    installs: number; price: string; code: string; longDesc: string;
    features: string[]; screenshots: Screenshot[]; tools: string[];
    worksWith: string[]; platforms: string[]; developer: Developer | null;
    pricing: unknown; ratingDistribution: Record<string, number> | null;
    requiredSkills: string[]; requiredPlugins: string[];
    usedBy: UsedByAgent[]; authorVerified: boolean;
    hasAuth: boolean; serverType: string; authType: string;
    reviews: ReviewItem[];
  }

  const pluginId = $derived($page.params.id ?? '');

  let apiProduct = $state<PluginDetail | null>(null);

  onMount(async () => {
    try {
      const [productRes, reviewsRes] = await Promise.all([
        getStoreProduct(pluginId) as Promise<{ app?: Record<string, unknown> } | null>,
        getStoreProductReviews(pluginId) as Promise<{ reviews?: Record<string, unknown>[] } | null>,
      ]);
      if (productRes?.app) {
        const a = productRes.app;
        const rawScreenshots = (a.screenshots || []) as Array<string | Record<string, unknown>>;
        apiProduct = {
          id: String(a.id ?? ''), name: String(a.name ?? ''), desc: String(a.description ?? ''),
          category: String(a.category ?? ''), rating: Number(a.rating ?? 0),
          installs: Number(a.installCount ?? 0), price: String(a.price ?? 'Get'), code: String(a.code ?? ''),
          longDesc: String(a.longDesc ?? ''), features: (a.features || []) as string[],
          screenshots: rawScreenshots.map(s => typeof s === 'string' ? { title: s, desc: '' } : { title: String((s as Record<string, unknown>).title ?? ''), desc: String((s as Record<string, unknown>).desc ?? '') }),
          tools: (a.tools || []) as string[], worksWith: (a.worksWith || []) as string[],
          platforms: (a.platforms || []) as string[], developer: a.developer ? a.developer as Developer : null,
          pricing: a.pricing ?? null, ratingDistribution: (a.ratingDistribution as Record<string, number>) ?? null,
          requiredSkills: (a.requiredSkills || []) as string[], requiredPlugins: (a.requiredPlugins || []) as string[],
          usedBy: (a.usedBy || []) as UsedByAgent[],
          authorVerified: Boolean((a.author as Record<string, unknown> | undefined)?.verified),
          hasAuth: Boolean(a.hasAuth), serverType: String(a.serverType ?? ''),
          authType: String(a.authType ?? ''), reviews: [],
        };
      }
      if (reviewsRes?.reviews?.length) {
        const mapped: ReviewItem[] = reviewsRes.reviews.map((r: Record<string, unknown>) => ({
          user: String(r.userName ?? ''), rating: Number(r.rating ?? 0),
          text: String(r.body ?? ''), date: String(r.createdAt ?? ''),
          role: String(r.role ?? ''), duration: String(r.duration ?? ''),
        }));
        if (apiProduct) apiProduct.reviews = mapped;
      }
    } catch {}
  });

  const detail = $derived(apiProduct);
  const plugin = $derived(apiProduct);
  const installed = $derived($installedIds.has(pluginId));

  let showOAuth = $state(false);
  let copied = $state(false);
  let activeScreenshot = $state(0);

  const usedByAgents = $derived(
    apiProduct?.usedBy ?? []
  );

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

  const totalReviews = $derived(detail?.ratingDistribution ? Object.values(detail.ratingDistribution).reduce((a, b) => a + b, 0) : (detail?.reviews?.length ?? 0));
  const maxRatingCount = $derived(detail?.ratingDistribution ? Math.max(...Object.values(detail.ratingDistribution)) : 1);

  function handleInstall() {
    if (installed) {
      uninstallItem(pluginId);
    } else if (plugin) {
      installItem({ id: pluginId, name: plugin.name, type: 'plugin' });
      if (detail?.hasAuth) {
        showOAuth = true;
      }
    }
  }

  function copyCode() {
    if (plugin?.code) {
      navigator.clipboard.writeText(plugin.code);
      copied = true;
      setTimeout(() => copied = false, 2000);
    }
  }
</script>

{#if plugin}
  <div class="p-6 max-w-[960px]">
    <a href="/marketplace/plugins" class="inline-flex items-center gap-1.5 text-xs text-base-content/50 hover:text-base-content transition-colors mb-5">
      <ArrowLeft class="w-3 h-3" /> Plugins
    </a>

    <div class="flex gap-8">
      <!-- Left sidebar -->
      <div class="w-[240px] shrink-0">
        <div class="w-16 h-16 rounded-2xl {getIconColor(pluginId)} grid place-items-center text-lg font-bold mb-3">{getInitials(plugin.name)}</div>
        <h1 class="text-lg font-semibold mb-1">{plugin.name}</h1>
        <div class="text-xs text-base-content/60 mb-3">{plugin.desc}</div>

        <div class="flex items-center gap-1.5 mb-1">
          <div class="flex items-center gap-0.5">
            {#each Array(5) as _, i}
              <Star class="w-3.5 h-3.5 {i < Math.round(plugin.rating) ? 'text-warning fill-warning' : 'text-base-content/20'}" />
            {/each}
          </div>
          <span class="text-xs font-medium">{plugin.rating}</span>
        </div>
        <div class="text-xs text-base-content/50 mb-4">{totalReviews.toLocaleString()} reviews · {plugin.installs?.toLocaleString()} installs</div>

        <div class="text-sm font-medium mb-4">{plugin.price === 'Get' ? 'Free' : plugin.price}</div>

        {#if detail?.authorVerified}
          <div class="flex items-center gap-1.5 mb-4">
            <ShieldCheck class="w-3.5 h-3.5 text-success" />
            <span class="text-xs font-medium text-success">Verified Publisher</span>
          </div>
        {/if}

        <button
          class="w-full py-2.5 px-4 rounded-lg text-sm font-medium cursor-pointer border-none transition-all mb-2.5 {installed ? 'bg-base-300 text-base-content hover:bg-base-content/10' : 'bg-primary text-primary-content hover:brightness-110'}"
          onclick={handleInstall}
        >{installed ? 'Uninstall' : plugin.price === 'Get' ? 'Install' : `Install · ${plugin.price}`}</button>

        {#if plugin.code}
          <button class="w-full flex items-center justify-center gap-1.5 py-2 px-3 rounded-lg border border-base-300 bg-base-100 text-xs font-mono text-base-content/60 cursor-pointer hover:text-base-content hover:border-base-content/20 transition-colors mb-5" onclick={copyCode}>
            {plugin.code}
            {#if copied}<Check class="w-3 h-3 text-success" />{:else}<Copy class="w-3 h-3" />{/if}
          </button>
        {/if}

        {#if detail?.developer}
          <div class="border-t border-base-300 pt-4">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2.5">Developer</div>
            <div class="text-sm font-medium mb-2">{detail.developer.name}</div>
            <div class="flex flex-col gap-1.5">
              <div class="flex items-center gap-1.5 text-xs text-base-content/50"><Globe class="w-3 h-3" /><span>{detail.developer.website}</span></div>
              <div class="flex items-center gap-1.5 text-xs text-base-content/50"><Mail class="w-3 h-3" /><span>{detail.developer.support}</span></div>
              <div class="flex items-center gap-1.5 text-xs text-base-content/50"><Calendar class="w-3 h-3" /><span>Launched {detail.developer.launched}</span></div>
            </div>
          </div>
        {/if}
      </div>

      <!-- Main content -->
      <div class="flex-1 min-w-0">
        {#if detail?.screenshots?.length}
          <div class="mb-8">
            <div class="relative rounded-xl overflow-hidden border border-base-300 bg-base-200 aspect-[16/9] mb-2.5">
              <div class="absolute inset-0 flex items-center justify-center text-center">
                <div>
                  <div class="text-sm font-medium text-base-content/70 mb-1">{detail.screenshots[activeScreenshot].title}</div>
                  <div class="text-xs text-base-content/40">{detail.screenshots[activeScreenshot].desc}</div>
                </div>
              </div>
              {#if detail.screenshots.length > 1}
                <button class="absolute left-2 top-1/2 -translate-y-1/2 w-8 h-8 rounded-full bg-base-100/80 border border-base-300 grid place-items-center cursor-pointer hover:bg-base-100 transition-colors" onclick={() => activeScreenshot = (activeScreenshot - 1 + detail.screenshots.length) % detail.screenshots.length}><ChevronLeft class="w-4 h-4" /></button>
                <button class="absolute right-2 top-1/2 -translate-y-1/2 w-8 h-8 rounded-full bg-base-100/80 border border-base-300 grid place-items-center cursor-pointer hover:bg-base-100 transition-colors" onclick={() => activeScreenshot = (activeScreenshot + 1) % detail.screenshots.length}><ChevronRight class="w-4 h-4" /></button>
              {/if}
            </div>
            {#if detail.screenshots.length > 1}
              <div class="flex items-center justify-center gap-1.5">
                {#each detail.screenshots as _, i}
                  <button class="w-2 h-2 rounded-full transition-colors cursor-pointer border-none {i === activeScreenshot ? 'bg-primary' : 'bg-base-content/20 hover:bg-base-content/40'}" onclick={() => activeScreenshot = i} aria-label="Go to screenshot {i + 1}"></button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}

        {#if detail?.longDesc}
          <div class="mb-8">
            <h2 class="text-sm font-semibold mb-2.5">About this plugin</h2>
            <div class="text-sm leading-relaxed text-base-content/80 mb-4">{detail.longDesc}</div>
            {#if detail?.features?.length}
              <ul class="flex flex-col gap-1.5">
                {#each detail.features as feature}
                  <li class="flex items-start gap-2 text-sm text-base-content/80"><Check class="w-4 h-4 text-success shrink-0 mt-0.5" />{feature}</li>
                {/each}
              </ul>
            {/if}
          </div>
        {/if}

        {#if detail?.platforms?.length}
          <div class="mb-8">
            <h2 class="text-sm font-semibold mb-2.5">Platforms</h2>
            <div class="flex flex-wrap gap-2">
              {#each detail.platforms as platform}
                <span class="inline-flex items-center gap-1.5 py-1.5 px-3 rounded-lg border border-base-300 bg-base-100 text-xs font-medium">
                  <Monitor class="w-3 h-3 text-base-content/40" />
                  {platform}
                </span>
              {/each}
            </div>
          </div>
        {/if}

        {#if detail?.worksWith?.length}
          <div class="mb-8">
            <h2 class="text-sm font-semibold mb-2.5">Works with</h2>
            <div class="flex flex-wrap gap-2">
              {#each detail.worksWith as integration}
                <span class="py-1.5 px-3 rounded-lg border border-base-300 bg-base-100 text-xs font-medium">{integration}</span>
              {/each}
            </div>
          </div>
        {/if}

        {#if usedByAgents.length}
          <div class="mb-8">
            <h2 class="text-sm font-semibold mb-2.5">Used by agents</h2>
            <div class="flex flex-col gap-2">
              {#each usedByAgents as agent}
                <a href="/marketplace/agents/{agent.id}" class="flex items-center gap-3 py-2.5 px-3.5 rounded-xl border border-base-300 bg-base-100 hover:shadow-sm hover:border-base-content/20 transition-all group">
                  <div class="w-8 h-8 rounded-lg {getIconColor(agent.id)} grid place-items-center text-xs font-bold shrink-0">{getInitials(agent.name)}</div>
                  <span class="text-sm font-medium flex-1 group-hover:text-primary transition-colors">{agent.name}</span>
                  <span class="py-0.5 px-2 rounded-full bg-base-200 text-xs text-base-content/50">{agent.category}</span>
                </a>
              {/each}
            </div>
          </div>
        {/if}

        {#if detail?.reviews?.length || detail?.ratingDistribution}
          <div>
            <h2 class="text-sm font-semibold mb-4">Reviews</h2>
            {#if detail?.ratingDistribution}
              <div class="flex gap-6 mb-6 p-4 rounded-xl border border-base-300 bg-base-100">
                <div class="text-center shrink-0">
                  <div class="text-3xl font-bold mb-1">{plugin.rating}</div>
                  <div class="flex items-center gap-0.5 justify-center mb-1">
                    {#each Array(5) as _, i}<Star class="w-3.5 h-3.5 {i < Math.round(plugin.rating) ? 'text-warning fill-warning' : 'text-base-content/20'}" />{/each}
                  </div>
                  <div class="text-xs text-base-content/50">{totalReviews.toLocaleString()} reviews</div>
                </div>
                <div class="flex-1 flex flex-col gap-1">
                  {#each [5, 4, 3, 2, 1] as stars}
                    {@const count = detail.ratingDistribution?.[String(stars)] ?? 0}
                    <div class="flex items-center gap-2">
                      <span class="text-xs text-base-content/50 w-3 text-right">{stars}</span>
                      <Star class="w-3 h-3 text-warning fill-warning shrink-0" />
                      <div class="flex-1 h-2 rounded-full bg-base-200 overflow-hidden"><div class="h-full rounded-full bg-warning transition-all" style="width: {(count / maxRatingCount) * 100}%"></div></div>
                      <span class="text-xs text-base-content/40 w-10 text-right">{count.toLocaleString()}</span>
                    </div>
                  {/each}
                </div>
              </div>
            {/if}
            {#if detail?.reviews?.length}
              <div class="flex flex-col gap-3">
                {#each detail.reviews as review}
                  <div class="p-4 rounded-xl border border-base-300 bg-base-100">
                    <div class="flex items-start gap-3 mb-2.5">
                      <div class="w-8 h-8 rounded-full bg-base-200 grid place-items-center text-xs font-bold shrink-0">{review.user[0]}</div>
                      <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2 mb-0.5">
                          <span class="text-sm font-medium">{review.user}</span>
                          <div class="flex items-center gap-0.5">{#each Array(5) as _, i}<Star class="w-3 h-3 {i < review.rating ? 'text-warning fill-warning' : 'text-base-content/20'}" />{/each}</div>
                          <span class="text-xs text-base-content/40 ml-auto shrink-0">{review.date}</span>
                        </div>
                        {#if review.role || review.duration}<div class="text-xs text-base-content/40 mb-1.5">{review.role ?? ''}{review.role && review.duration ? ' · ' : ''}{review.duration ?? ''}</div>{/if}
                      </div>
                    </div>
                    <div class="text-sm leading-relaxed text-base-content/80 pl-11">{review.text}</div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
{:else}
  <div class="flex-1 flex items-center justify-center text-sm text-base-content/50">Plugin not found</div>
{/if}

<OAuthConnectModal bind:show={showOAuth} pluginName={plugin?.name ?? 'Plugin'} />

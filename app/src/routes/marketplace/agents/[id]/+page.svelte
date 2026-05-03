<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { getStoreProduct, getStoreProductReviews } from '$lib/api/index';
  import { installedIds, installItem, uninstallItem } from '$lib/stores/marketplace.js';
  import AgentSetupModal from '$lib/components/AgentSetupModal.svelte';
  import Star from 'lucide-svelte/icons/star';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import ShieldCheck from 'lucide-svelte/icons/shield-check';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import Play from 'lucide-svelte/icons/play';
  import Globe from 'lucide-svelte/icons/globe';
  import Mail from 'lucide-svelte/icons/mail';
  import Calendar from 'lucide-svelte/icons/calendar';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';

  const agentId = $derived($page.params.id);

  let apiProduct = $state<Record<string, unknown> | null>(null);
  let apiReviews = $state<Record<string, unknown>[]>([]);

  onMount(async () => {
    try {
      const [productRes, reviewsRes] = await Promise.all([
        getStoreProduct(agentId),
        getStoreProductReviews(agentId),
      ]);
      if (productRes?.app) {
        const a = productRes.app as Record<string, unknown>;
        apiProduct = {
          id: a.id, name: a.name, desc: a.description || '',
          category: a.category || '', rating: a.rating || 0,
          installs: a.installCount || 0, price: a.price || 'Get', code: a.code || '',
          longDesc: a.longDesc || '', features: a.features || [],
          screenshots: ((a.screenshots || []) as Record<string, unknown>[]).map((s: Record<string, unknown>) => typeof s === 'string' ? { title: s, desc: '' } : s),
          tools: a.tools || [], worksWith: a.worksWith || [],
          platforms: a.platforms || [], developer: a.developer || null,
          pricing: a.pricing || null, ratingDistribution: a.ratingDistribution || null,
          requiredSkills: a.requiredSkills || [], requiredPlugins: a.requiredPlugins || [],
          usedBy: a.usedBy || [], authorVerified: (a.author as Record<string, unknown>)?.verified ?? false,
          videoUrl: a.videoUrl || '', reviews: [] as Record<string, unknown>[],
        };
      }
      if (reviewsRes?.reviews?.length) {
        apiReviews = reviewsRes.reviews.map((r: Record<string, unknown>) => ({
          user: r.userName || '', rating: r.rating || 0,
          text: r.body || '', date: r.createdAt || '',
          role: r.role || '', duration: r.duration || '',
        }));
        if (apiProduct) apiProduct.reviews = apiReviews;
      }
    } catch {}
  });

  const detail = $derived(apiProduct);
  const agent = $derived(apiProduct || null);
  const installed = $derived($installedIds.has(agentId));

  let showSetup = $state(false);
  let copied = $state(false);
  let activeScreenshot = $state(0);

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

  // Rating summary
  const totalReviews = $derived(detail?.ratingDistribution ? Object.values(detail.ratingDistribution as Record<string, number>).reduce((a: number, b: number) => a + b, 0) : (detail?.reviews?.length ?? 0));
  const maxRatingCount = $derived(detail?.ratingDistribution ? Math.max(...Object.values(detail.ratingDistribution as Record<string, number>)) : 1);

  function handleInstall() {
    if (installed) {
      uninstallItem(agentId);
    } else {
      installItem({ id: agentId, name: agent.name, type: 'agent' });
      showSetup = true;
    }
  }

  function copyCode() {
    if (agent?.code) {
      navigator.clipboard.writeText(agent.code);
      copied = true;
      setTimeout(() => copied = false, 2000);
    }
  }
</script>

{#if agent}
  <div class="p-6 max-w-[960px]">
    <a href="/marketplace/agents" class="inline-flex items-center gap-1.5 text-xs text-base-content/50 hover:text-base-content transition-colors mb-5">
      <ArrowLeft class="w-3 h-3" />
      Agents
    </a>

    <!-- Two-column layout -->
    <div class="flex gap-8">
      <!-- Left sidebar -->
      <div class="w-[240px] shrink-0">
        <!-- Icon + name -->
        <div class="w-16 h-16 rounded-2xl {getIconColor(agentId)} grid place-items-center text-lg font-bold mb-3">
          {getInitials(agent.name)}
        </div>
        <h1 class="text-lg font-semibold mb-1">{agent.name}</h1>
        <div class="text-xs text-base-content/60 mb-3">{agent.desc}</div>

        <!-- Rating -->
        <div class="flex items-center gap-1.5 mb-1">
          <div class="flex items-center gap-0.5">
            {#each Array(5) as _, i}
              <Star class="w-3.5 h-3.5 {i < Math.round(agent.rating) ? 'text-warning fill-warning' : 'text-base-content/20'}" />
            {/each}
          </div>
          <span class="text-xs font-medium">{agent.rating}</span>
        </div>
        <div class="text-xs text-base-content/50 mb-4">{totalReviews.toLocaleString()} reviews · {agent.installs?.toLocaleString()} installs</div>

        <!-- Price -->
        {#if detail?.pricing}
          <div class="text-sm font-medium mb-0.5">
            {detail.pricing[0].price === 'Free' ? 'Free' : `From ${detail.pricing[0].price}`}
          </div>
          {#if detail.pricing[0].trial}
            <div class="text-xs text-base-content/50 mb-4">{detail.pricing[0].trial}</div>
          {/if}
        {:else}
          <div class="text-sm font-medium mb-4">{agent.price === 'Get' ? 'Free' : agent.price}</div>
        {/if}

        {#if detail?.authorVerified}
          <div class="flex items-center gap-1.5 mb-4">
            <ShieldCheck class="w-3.5 h-3.5 text-success" />
            <span class="text-xs font-medium text-success">Verified Publisher</span>
          </div>
        {/if}

        <!-- Install button -->
        <button
          class="w-full py-2.5 px-4 rounded-lg text-sm font-medium cursor-pointer border-none transition-all mb-2.5 {installed ? 'bg-base-300 text-base-content hover:bg-base-content/10' : 'bg-primary text-primary-content hover:brightness-110'}"
          onclick={handleInstall}
        >{installed ? 'Uninstall' : agent.price === 'Get' ? 'Install' : `Install · ${agent.price}`}</button>

        {#if agent.code}
          <button
            class="w-full flex items-center justify-center gap-1.5 py-2 px-3 rounded-lg border border-base-300 bg-base-100 text-xs font-mono text-base-content/60 cursor-pointer hover:text-base-content hover:border-base-content/20 transition-colors mb-5"
            onclick={copyCode}
          >
            {agent.code}
            {#if copied}
              <Check class="w-3 h-3 text-success" />
            {:else}
              <Copy class="w-3 h-3" />
            {/if}
          </button>
        {/if}

        <!-- Developer info -->
        {#if detail?.developer}
          <div class="border-t border-base-300 pt-4">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2.5">Developer</div>
            <div class="text-sm font-medium mb-2">{detail.developer.name}</div>
            <div class="flex flex-col gap-1.5">
              <div class="flex items-center gap-1.5 text-xs text-base-content/50">
                <Globe class="w-3 h-3" />
                <span>{detail.developer.website}</span>
              </div>
              <div class="flex items-center gap-1.5 text-xs text-base-content/50">
                <Mail class="w-3 h-3" />
                <span>{detail.developer.support}</span>
              </div>
              <div class="flex items-center gap-1.5 text-xs text-base-content/50">
                <Calendar class="w-3 h-3" />
                <span>Launched {detail.developer.launched}</span>
              </div>
            </div>
          </div>
        {/if}
      </div>

      <!-- Main content -->
      <div class="flex-1 min-w-0">
        <!-- Screenshot gallery -->
        {#if detail?.screenshots?.length}
          <div class="mb-8">
            <div class="relative rounded-xl overflow-hidden border border-base-300 bg-base-200 aspect-[16/9] mb-2.5">
              <div class="absolute inset-0 flex items-center justify-center">
                <div class="text-center">
                  <div class="text-sm font-medium text-base-content/70 mb-1">{detail.screenshots[activeScreenshot].title}</div>
                  <div class="text-xs text-base-content/40">{detail.screenshots[activeScreenshot].desc}</div>
                </div>
              </div>
              {#if detail.screenshots.length > 1}
                <button
                  class="absolute left-2 top-1/2 -translate-y-1/2 w-8 h-8 rounded-full bg-base-100/80 border border-base-300 grid place-items-center cursor-pointer hover:bg-base-100 transition-colors"
                  onclick={() => activeScreenshot = (activeScreenshot - 1 + detail.screenshots.length) % detail.screenshots.length}
                >
                  <ChevronLeft class="w-4 h-4" />
                </button>
                <button
                  class="absolute right-2 top-1/2 -translate-y-1/2 w-8 h-8 rounded-full bg-base-100/80 border border-base-300 grid place-items-center cursor-pointer hover:bg-base-100 transition-colors"
                  onclick={() => activeScreenshot = (activeScreenshot + 1) % detail.screenshots.length}
                >
                  <ChevronRight class="w-4 h-4" />
                </button>
              {/if}
            </div>
            <!-- Thumbnail dots -->
            {#if detail.screenshots.length > 1}
              <div class="flex items-center justify-center gap-1.5">
                {#each detail.screenshots as _, i}
                  <button
                    class="w-2 h-2 rounded-full transition-colors cursor-pointer border-none {i === activeScreenshot ? 'bg-primary' : 'bg-base-content/20 hover:bg-base-content/40'}"
                    onclick={() => activeScreenshot = i}
                  ></button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}

        <!-- Video -->
        {#if detail?.videoUrl}
          <div class="mb-8">
            <div class="rounded-xl border border-base-300 bg-base-200 aspect-video flex items-center justify-center cursor-pointer hover:bg-base-200/80 transition-colors group">
              <div class="w-14 h-14 rounded-full bg-base-100/80 border border-base-300 grid place-items-center group-hover:scale-110 transition-transform">
                <Play class="w-6 h-6 text-primary ml-0.5" />
              </div>
            </div>
          </div>
        {/if}

        <!-- Description + features -->
        {#if detail?.longDesc}
          <div class="mb-8">
            <h2 class="text-sm font-semibold mb-2.5">About this agent</h2>
            <div class="text-sm leading-relaxed text-base-content/80 mb-4">{detail.longDesc}</div>
            {#if detail?.features?.length}
              <ul class="flex flex-col gap-1.5">
                {#each detail.features as feature}
                  <li class="flex items-start gap-2 text-sm text-base-content/80">
                    <Check class="w-4 h-4 text-success shrink-0 mt-0.5" />
                    {feature}
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/if}

        <!-- Works with -->
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

        <!-- Required skills/plugins -->
        {#if detail?.requiredSkills?.length || detail?.requiredPlugins?.length}
          <div class="mb-8">
            <h2 class="text-sm font-semibold mb-2.5">Requirements</h2>
            <div class="flex flex-col gap-2">
              {#if detail?.requiredSkills?.length}
                {#each detail.requiredSkills as skill}
                  <a href="/marketplace/skills/{skill.id}" class="flex items-center gap-3 py-2.5 px-3.5 rounded-xl border border-base-300 bg-base-100 hover:shadow-sm hover:border-base-content/20 transition-all group">
                    <div class="w-8 h-8 rounded-lg {getIconColor(skill.id)} grid place-items-center text-xs font-bold shrink-0">{getInitials(skill.name)}</div>
                    <div class="flex-1 min-w-0">
                      <span class="text-sm font-medium group-hover:text-primary transition-colors">{skill.name}</span>
                      <span class="text-xs text-base-content/40 ml-2">Skill</span>
                    </div>
                    {#if $installedIds.has(skill.id)}
                      <span class="text-xs font-medium text-success">Installed</span>
                    {:else}
                      <span class="text-xs text-base-content/40">Required</span>
                    {/if}
                  </a>
                {/each}
              {/if}
              {#if detail?.requiredPlugins?.length}
                {#each detail.requiredPlugins as plugin}
                  <a href="/marketplace/plugins/{plugin.id}" class="flex items-center gap-3 py-2.5 px-3.5 rounded-xl border border-base-300 bg-base-100 hover:shadow-sm hover:border-base-content/20 transition-all group">
                    <div class="w-8 h-8 rounded-lg {getIconColor(plugin.id)} grid place-items-center text-xs font-bold shrink-0">{getInitials(plugin.name)}</div>
                    <div class="flex-1 min-w-0">
                      <span class="text-sm font-medium group-hover:text-primary transition-colors">{plugin.name}</span>
                      <span class="text-xs text-base-content/40 ml-2">Plugin</span>
                    </div>
                    {#if $installedIds.has(plugin.id)}
                      <span class="text-xs font-medium text-success">Installed</span>
                    {:else}
                      <span class="text-xs text-base-content/40">Required</span>
                    {/if}
                  </a>
                {/each}
              {/if}
            </div>
          </div>
        {/if}

        <!-- Pricing tiers -->
        {#if detail?.pricing?.length}
          <div class="mb-8">
            <h2 class="text-sm font-semibold mb-3">Pricing</h2>
            <div class="grid grid-cols-{detail.pricing.length} gap-3">
              {#each detail.pricing as tier}
                <div class="p-4 rounded-xl border {tier.popular ? 'border-primary bg-primary/5' : 'border-base-300 bg-base-100'} relative">
                  {#if tier.popular}
                    <div class="absolute -top-2.5 left-1/2 -translate-x-1/2 py-0.5 px-2.5 rounded-full bg-primary text-primary-content text-xs font-medium">Popular</div>
                  {/if}
                  <div class="text-sm font-semibold mb-0.5">{tier.name}</div>
                  <div class="text-lg font-bold mb-0.5">{tier.price}</div>
                  {#if tier.annual && tier.price !== 'Custom'}
                    <div class="text-xs text-base-content/50 mb-3">or {tier.annual} billed annually</div>
                  {:else if tier.trial}
                    <div class="text-xs text-base-content/50 mb-3">{tier.trial}</div>
                  {/if}
                  <ul class="flex flex-col gap-1">
                    {#each tier.features as feature}
                      <li class="flex items-start gap-1.5 text-xs text-base-content/70">
                        <Check class="w-3 h-3 text-success shrink-0 mt-0.5" />
                        {feature}
                      </li>
                    {/each}
                  </ul>
                </div>
              {/each}
            </div>
          </div>
        {/if}

        <!-- Reviews -->
        {#if detail?.reviews?.length || detail?.ratingDistribution}
          <div>
            <h2 class="text-sm font-semibold mb-4">Reviews</h2>

            <!-- Rating summary -->
            {#if detail?.ratingDistribution}
              <div class="flex gap-6 mb-6 p-4 rounded-xl border border-base-300 bg-base-100">
                <!-- Overall score -->
                <div class="text-center shrink-0">
                  <div class="text-3xl font-bold mb-1">{agent.rating}</div>
                  <div class="flex items-center gap-0.5 justify-center mb-1">
                    {#each Array(5) as _, i}
                      <Star class="w-3.5 h-3.5 {i < Math.round(agent.rating) ? 'text-warning fill-warning' : 'text-base-content/20'}" />
                    {/each}
                  </div>
                  <div class="text-xs text-base-content/50">{totalReviews.toLocaleString()} reviews</div>
                </div>
                <!-- Distribution bars -->
                <div class="flex-1 flex flex-col gap-1">
                  {#each [5, 4, 3, 2, 1] as stars}
                    {@const count = (detail.ratingDistribution as Record<number, number>)[stars] ?? 0}
                    <div class="flex items-center gap-2">
                      <span class="text-xs text-base-content/50 w-3 text-right">{stars}</span>
                      <Star class="w-3 h-3 text-warning fill-warning shrink-0" />
                      <div class="flex-1 h-2 rounded-full bg-base-200 overflow-hidden">
                        <div class="h-full rounded-full bg-warning transition-all" style="width: {(count / maxRatingCount) * 100}%"></div>
                      </div>
                      <span class="text-xs text-base-content/40 w-10 text-right">{count.toLocaleString()}</span>
                    </div>
                  {/each}
                </div>
              </div>
            {/if}

            <!-- Individual reviews -->
            {#if detail?.reviews?.length}
              <div class="flex flex-col gap-3">
                {#each detail.reviews as review}
                  <div class="p-4 rounded-xl border border-base-300 bg-base-100">
                    <div class="flex items-start gap-3 mb-2.5">
                      <div class="w-8 h-8 rounded-full bg-base-200 grid place-items-center text-xs font-bold shrink-0">{review.user[0]}</div>
                      <div class="flex-1 min-w-0">
                        <div class="flex items-center gap-2 mb-0.5">
                          <span class="text-sm font-medium">{review.user}</span>
                          <div class="flex items-center gap-0.5">
                            {#each Array(5) as _, i}
                              <Star class="w-3 h-3 {i < review.rating ? 'text-warning fill-warning' : 'text-base-content/20'}" />
                            {/each}
                          </div>
                          <span class="text-xs text-base-content/40 ml-auto shrink-0">{review.date}</span>
                        </div>
                        {#if review.role || review.duration}
                          <div class="text-xs text-base-content/40 mb-1.5">
                            {review.role ?? ''}{review.role && review.duration ? ' · ' : ''}{review.duration ?? ''}
                          </div>
                        {/if}
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
  <div class="flex-1 flex items-center justify-center text-sm text-base-content/50">Agent not found</div>
{/if}

<AgentSetupModal bind:show={showSetup} agentName={agent?.name ?? 'Agent'} />

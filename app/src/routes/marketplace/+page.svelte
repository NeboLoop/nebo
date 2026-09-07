<!--
  The marketplace as a page. Deep links from install codes, "View all" and
  product cards land here; the shelf opens the same browse view as a modal
  over the workspace. One component, two mounts.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { t } from 'svelte-i18n';
  import Search from 'lucide-svelte/icons/search';
  import MarketplaceBrowse from '$lib/components/marketplace/MarketplaceBrowse.svelte';

  // Employees is the default view (same as the website); the legacy
  // category/publisher storefronts linked from cards still resolve as 'all'.
  const kind = $derived(
    $page.url.searchParams.get('kind') ||
      ($page.url.searchParams.get('category') || $page.url.searchParams.get('publisher') ? 'all' : 'employees')
  );
  const price = $derived($page.url.searchParams.get('price') || 'all');
  const category = $derived($page.url.searchParams.get('category') || '');
  const publisher = $derived($page.url.searchParams.get('publisher') || '');
  const filter = $derived($page.url.searchParams.get('filter') || '');
  // The deep link may carry a query; the box below is the same ONE search
  // the storefront modal has.
  let q = $state($page.url.searchParams.get('q') || '');
</script>

<svelte:head><title>Marketplace - Nebo</title></svelte:head>

<div class="max-w-6xl mx-auto px-6 pt-6">
  <label class="w-80 max-w-full flex items-center gap-2 rounded-full border border-base-300 bg-base-100 px-3 py-1.5 focus-within:border-primary">
    <Search class="w-4 h-4 text-base-content/50 shrink-0" />
    <input class="w-full bg-transparent outline-none text-sm" type="search" placeholder={$t('marketplace.searchAllPlaceholder')} bind:value={q} />
  </label>
</div>
<MarketplaceBrowse {kind} {price} {category} {publisher} {filter} {q} />

<!--
  The marketplace as a page. Deep links from install codes, "View all" and
  product cards land here; the shelf opens the same browse view as a modal
  over the workspace. One component, two mounts.
-->
<script lang="ts">
  import { page } from '$app/stores';
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
</script>

<MarketplaceBrowse {kind} {price} {category} {publisher} />

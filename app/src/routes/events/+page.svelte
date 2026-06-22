<script lang="ts">
  // System Events moved into Settings → Developer (System Events). Redirect any
  // old /events link to its new canonical home so there's a single page.
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  onMount(() => goto('/settings/events', { replaceState: true }));
</script>

<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';

  onMount(() => {
    goto('/assistant/threads', { replaceState: true });
  });
</script>

<div class="flex items-center justify-center h-full">
  <span class="loading loading-spinner loading-md"></span>
</div>

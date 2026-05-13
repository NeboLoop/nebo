<script lang="ts">
  import type { Snippet } from 'svelte';
  let { children }: { children: Snippet } = $props();
</script>

<div class="fixed inset-0 z-[60] flex items-center justify-center bg-base-100 overflow-y-auto">
  <div class="w-full max-w-2xl px-6 py-10">
    {@render children()}
  </div>
</div>

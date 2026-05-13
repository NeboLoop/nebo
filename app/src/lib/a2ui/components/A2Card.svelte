<script lang="ts">
  import type { Snippet } from 'svelte';

  let { class: className = '', children }: {
    class?: string;
    children?: Snippet;
  } = $props();
</script>

<div class="rounded-box border border-base-300 p-4 {className}">
  {#if children}{@render children()}{/if}
</div>

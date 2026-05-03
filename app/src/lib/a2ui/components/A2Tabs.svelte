<script lang="ts">
  import type { Snippet } from 'svelte';

  let { tabs = [], activeTab = '', ontabchange, children }: {
    tabs?: { id: string; label: string }[];
    activeTab?: string;
    ontabchange?: (tabId: string) => void;
    children?: Snippet;
  } = $props();
</script>

<div class="flex flex-col gap-3">
  <div class="flex gap-1">
    {#each tabs as tab}
      <button
        class="py-1.5 px-3 rounded-md text-sm cursor-pointer border-none transition-colors {activeTab === tab.id ? 'bg-base-100 shadow-[0_0_0_1px_var(--color-base-300)] font-medium text-base-content' : 'bg-transparent hover:bg-base-100/70 text-base-content/70'}"
        onclick={() => ontabchange?.(tab.id)}
      >{tab.label}</button>
    {/each}
  </div>
  {#if children}{@render children()}{/if}
</div>

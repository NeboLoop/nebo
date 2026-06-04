<script lang="ts">
  import type { Snippet } from 'svelte';

  // The single settings page header. Every settings page uses this so the
  // title size, weight, spacing, and subtitle are identical everywhere.
  // Optional `action` renders a right-aligned control (e.g. a button) in the header row.
  let {
    title,
    description,
    action,
  }: {
    title: string;
    description: string;
    action?: Snippet;
  } = $props();
</script>

<div class="mb-7">
  <div class="flex items-center justify-between gap-3">
    <div>
      <h2 class="text-lg font-bold mb-1">{title}</h2>
      <p class="text-xs text-base-content/70">{description}</p>
    </div>
    {#if action}
      <div class="flex items-center gap-2 shrink-0">{@render action()}</div>
    {/if}
  </div>
</div>

<script lang="ts">
  import type { Snippet } from 'svelte';

  // A list row matching the MCP Servers "Configured Servers" rows:
  // optional leading indicator (status dot / avatar), main content, trailing actions.
  let {
    leading,
    children,
    actions,
  }: {
    leading?: Snippet;
    children: Snippet;
    actions?: Snippet;
  } = $props();
</script>

<div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-300 bg-base-100">
  {#if leading}{@render leading()}{/if}
  <div class="flex-1 min-w-0">{@render children()}</div>
  {#if actions}<div class="flex items-center gap-1.5 shrink-0">{@render actions()}</div>{/if}
</div>

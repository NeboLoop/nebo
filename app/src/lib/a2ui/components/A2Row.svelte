<script lang="ts">
  import type { Snippet } from 'svelte';

  let { justify = 'start', align = 'stretch', gap = '3', wrap = false, class: className = '', children }: {
    justify?: string;
    align?: string;
    gap?: string;
    wrap?: boolean;
    class?: string;
    children?: Snippet;
  } = $props();

  const justifyMap: Record<string, string> = {
    start: 'justify-start', center: 'justify-center', end: 'justify-end',
    between: 'justify-between', around: 'justify-around', evenly: 'justify-evenly',
  };

  const alignMap: Record<string, string> = {
    start: 'items-start', center: 'items-center', end: 'items-end', stretch: 'items-stretch',
  };

  const gapMap: Record<string, string> = {
    '0': 'gap-0', '1': 'gap-1', '2': 'gap-2', '3': 'gap-3', '4': 'gap-4', '5': 'gap-5', '6': 'gap-6',
  };
</script>

<div class="flex flex-row {gapMap[gap] ?? 'gap-3'} {justifyMap[justify] ?? ''} {alignMap[align] ?? ''} {wrap ? 'flex-wrap' : ''} {className}">
  {#if children}{@render children()}{/if}
</div>

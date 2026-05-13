<script lang="ts">
  import { N } from '$lib/tokens.js';
  let { kind, size = 6 } = $props();

  const color = $derived(
    kind === 'running' ? N.active
    : kind === 'review'  ? N.needs
    : kind === 'blocked' ? N.blocked
    : N.idle
  );
  const pulse = $derived(kind === 'running' || kind === 'review');
</script>

<span
  class="status-dot"
  style:width="{size}px"
  style:height="{size}px"
  style:background={color}
>
  {#if pulse}
    <span class="status-dot__pulse" style:background={color}></span>
  {/if}
</span>

<script lang="ts">
  let { url = '', description = '', fit = 'cover', class: cls = '' }: {
    url?: string;
    description?: string;
    fit?: string;
    class?: string;
  } = $props();
</script>

{#if url}
  <img
    src={url}
    alt={description}
    class="rounded-lg max-w-full {fit === 'contain' ? 'object-contain' : 'object-cover'} {cls}"
  />
{/if}

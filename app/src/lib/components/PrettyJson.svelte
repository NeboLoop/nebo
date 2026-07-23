<script lang="ts">
  // Recursive, human-friendly renderer for arbitrary JSON (run inputs, event
  // payloads, tool results). Keys are muted mono labels; values are colored by
  // type; nested objects/arrays indent. Callers pair this with a Raw toggle that
  // swaps in a <pre> of the source — this is the "pretty" default.
  import Self from './PrettyJson.svelte';

  let { value }: { value: unknown } = $props();

  function kind(v: unknown): 'object' | 'array' | 'string' | 'number' | 'boolean' | 'null' {
    if (v === null || v === undefined) return 'null';
    if (Array.isArray(v)) return 'array';
    const t = typeof v;
    return t === 'object' ? 'object' : (t as 'string' | 'number' | 'boolean');
  }

  const k = $derived(kind(value));
  const entries = $derived(k === 'object' ? Object.entries(value as Record<string, unknown>) : []);
  const items = $derived(k === 'array' ? (value as unknown[]) : []);
</script>

{#if k === 'object'}
  {#if entries.length === 0}
    <span class="text-xs font-mono text-base-content/40">{'{ }'}</span>
  {:else}
    <div class="flex flex-col gap-1">
      {#each entries as [key, v] (key)}
        <div class="flex gap-2 items-baseline">
          <span class="text-xs font-mono font-medium text-base-content/50 shrink-0">{key}</span>
          <div class="min-w-0 flex-1"><Self value={v} /></div>
        </div>
      {/each}
    </div>
  {/if}
{:else if k === 'array'}
  {#if items.length === 0}
    <span class="text-xs font-mono text-base-content/40">[ ]</span>
  {:else}
    <div class="flex flex-col gap-1 border-l border-base-content/10 pl-2">
      {#each items as v, i (i)}
        <Self value={v} />
      {/each}
    </div>
  {/if}
{:else if k === 'string'}
  <span class="text-xs break-words text-base-content/80">{value}</span>
{:else if k === 'null'}
  <span class="text-xs font-mono text-base-content/40">null</span>
{:else}
  <span class="text-xs font-mono text-info">{String(value)}</span>
{/if}

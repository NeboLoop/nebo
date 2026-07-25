<script lang="ts">
  // Recursive, human-friendly renderer for arbitrary JSON (run inputs, event
  // payloads, tool results). Scalar values render inline after their key and
  // wrap at the full panel width; nested objects/arrays put the key on its own
  // line with an indented block below — keys and values never compete for the
  // same row, so deep nesting can't squeeze values into a sliver. Callers pair
  // this with a Raw toggle that swaps in a <pre> of the source.
  import Self from './PrettyJson.svelte';

  let { value }: { value: unknown } = $props();

  function kind(v: unknown): 'object' | 'array' | 'string' | 'number' | 'boolean' | 'null' {
    if (v === null || v === undefined) return 'null';
    if (Array.isArray(v)) return 'array';
    const t = typeof v;
    return t === 'object' ? 'object' : (t as 'string' | 'number' | 'boolean');
  }

  function isNested(v: unknown): boolean {
    const t = kind(v);
    if (t === 'object') return Object.keys(v as Record<string, unknown>).length > 0;
    if (t === 'array') return (v as unknown[]).length > 0;
    return false;
  }

  const k = $derived(kind(value));
  const entries = $derived(k === 'object' ? Object.entries(value as Record<string, unknown>) : []);
  const items = $derived(k === 'array' ? (value as unknown[]) : []);
</script>

{#if k === 'object'}
  {#if entries.length === 0}
    <span class="text-xs font-mono text-base-content/40">{'{ }'}</span>
  {:else}
    <div class="flex flex-col gap-1.5 min-w-0">
      {#each entries as [key, v] (key)}
        {#if isNested(v)}
          <div class="min-w-0">
            <div class="text-xs font-mono font-medium text-base-content/50">{key}</div>
            <div class="mt-1 pl-3 border-l border-base-content/10 min-w-0"><Self value={v} /></div>
          </div>
        {:else}
          <div class="text-xs min-w-0 break-words">
            <span class="font-mono font-medium text-base-content/50">{key}</span>
            <Self value={v} />
          </div>
        {/if}
      {/each}
    </div>
  {/if}
{:else if k === 'array'}
  {#if items.length === 0}
    <span class="text-xs font-mono text-base-content/40">[ ]</span>
  {:else}
    <div class="flex flex-col gap-2 divide-y divide-base-content/10 min-w-0">
      {#each items as v, i (i)}
        <div class="min-w-0 [&:not(:first-child)]:pt-2"><Self value={v} /></div>
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

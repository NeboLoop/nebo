<script lang="ts">
  import type { A2UIComponentDef } from './types.js';
  import { resolveValue, resolveArray } from './resolve.js';
  import A2Column from './components/A2Column.svelte';
  import A2Row from './components/A2Row.svelte';
  import A2Text from './components/A2Text.svelte';
  import A2Button from './components/A2Button.svelte';
  import A2Card from './components/A2Card.svelte';
  import A2Divider from './components/A2Divider.svelte';
  import A2Icon from './components/A2Icon.svelte';
  import A2Image from './components/A2Image.svelte';
  import A2Tabs from './components/A2Tabs.svelte';

  let { def, rootData, scopeData, components, onaction }: {
    def: A2UIComponentDef;
    rootData: Record<string, unknown>;
    scopeData?: Record<string, unknown>;
    components: Map<string, A2UIComponentDef>;
    onaction?: (name: string, payload?: Record<string, unknown>) => void;
  } = $props();

  // Resolve props that might contain TextValue bindings
  function resolveProp(val: unknown): string {
    if (val === undefined || val === null) return '';
    if (typeof val === 'string') return val;
    if (typeof val === 'object' && 'path' in (val as Record<string, unknown>)) {
      return resolveValue(val as { path: string }, rootData, scopeData);
    }
    return String(val);
  }

  // Get child component defs for static children (string[] of IDs)
  function getStaticChildren(): A2UIComponentDef[] {
    if (!def.children || !Array.isArray(def.children)) return [];
    return (def.children as string[])
      .map(id => components.get(id))
      .filter((c): c is A2UIComponentDef => c !== undefined);
  }

  // Get list items for template children ({ componentId, path })
  function getListItems(): { template: A2UIComponentDef; items: Record<string, unknown>[] } | null {
    if (!def.children || Array.isArray(def.children)) return null;
    const ref = def.children as { componentId: string; path: string };
    const template = components.get(ref.componentId);
    if (!template) return null;
    const items = resolveArray(ref.path, rootData, scopeData) as Record<string, unknown>[];
    return { template, items };
  }

  const isListTemplate = $derived(def.children && !Array.isArray(def.children));
  const staticChildren = $derived(getStaticChildren());
  const listData = $derived(isListTemplate ? getListItems() : null);

  const p = $derived(def.props ?? {});
</script>

{#if def.component === 'column'}
  <A2Column
    justify={resolveProp(p.justify) || undefined}
    align={resolveProp(p.align) || undefined}
    gap={resolveProp(p.gap) || undefined}
    class={resolveProp(p.class)}
  >
    {#if listData}
      {#each listData.items as item}
        <svelte:self def={listData.template} {rootData} scopeData={item} {components} {onaction} />
      {/each}
    {:else}
      {#each staticChildren as child}
        <svelte:self def={child} {rootData} {scopeData} {components} {onaction} />
      {/each}
    {/if}
  </A2Column>

{:else if def.component === 'row'}
  <A2Row
    justify={resolveProp(p.justify) || undefined}
    align={resolveProp(p.align) || undefined}
    gap={resolveProp(p.gap) || undefined}
    wrap={p.wrap === true}
    class={resolveProp(p.class)}
  >
    {#if listData}
      {#each listData.items as item}
        <svelte:self def={listData.template} {rootData} scopeData={item} {components} {onaction} />
      {/each}
    {:else}
      {#each staticChildren as child}
        <svelte:self def={child} {rootData} {scopeData} {components} {onaction} />
      {/each}
    {/if}
  </A2Row>

{:else if def.component === 'text'}
  <A2Text
    text={resolveProp(p.text)}
    variant={resolveProp(p.variant) || undefined}
    class={resolveProp(p.class)}
  />

{:else if def.component === 'button'}
  <A2Button
    label={resolveProp(p.label)}
    variant={resolveProp(p.variant) || undefined}
    size={resolveProp(p.size) || undefined}
    action={def.action}
    {onaction}
  />

{:else if def.component === 'card'}
  <A2Card class={resolveProp(p.class)}>
    {#if listData}
      {#each listData.items as item}
        <svelte:self def={listData.template} {rootData} scopeData={item} {components} {onaction} />
      {/each}
    {:else}
      {#each staticChildren as child}
        <svelte:self def={child} {rootData} {scopeData} {components} {onaction} />
      {/each}
    {/if}
  </A2Card>

{:else if def.component === 'tabs'}
  {@const tabDefs = (p.tabs as { id: string; label: string }[]) || []}
  <A2Tabs tabs={tabDefs} activeTab={tabDefs[0]?.id || ''}>
    {#snippet children(activeTabId)}
      {#each staticChildren as child}
        {#if child.id === activeTabId}
          <svelte:self def={child} {rootData} {scopeData} {components} {onaction} />
        {/if}
      {/each}
    {/snippet}
  </A2Tabs>

{:else if def.component === 'divider'}
  <A2Divider />

{:else if def.component === 'icon'}
  <A2Icon
    name={resolveProp(p.name)}
    size={p.size ? Number(p.size) : undefined}
    class={resolveProp(p.class)}
  />

{:else if def.component === 'image'}
  <A2Image
    url={resolveProp(p.url)}
    description={resolveProp(p.description)}
    fit={resolveProp(p.fit) || undefined}
    class={resolveProp(p.class)}
  />

{:else if def.component === 'badge'}
  {@const badgeVariant = resolveProp(p.variant)}
  {@const badgeClasses = badgeVariant === 'success' ? 'bg-[var(--agent-green-bg)] text-[var(--agent-green-ink)]'
    : badgeVariant === 'warning' ? 'bg-[var(--agent-amber-bg)] text-[var(--agent-amber-ink)]'
    : badgeVariant === 'error' ? 'bg-error/10 text-error'
    : badgeVariant === 'info' ? 'bg-[var(--agent-sky-bg)] text-[var(--agent-sky-ink)]'
    : badgeVariant === 'accent' ? 'bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)]'
    : 'bg-base-300'}
  <span class="py-0.5 px-2 rounded-full text-sm font-medium {badgeClasses}">{resolveProp(p.text)}</span>

{:else if def.component === 'stat'}
  <div class="rounded-xl bg-base-100 shadow-sm p-4 {resolveProp(p.class)}">
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{resolveProp(p.label)}</div>
    <div class="text-2xl font-semibold tracking-tight">{resolveProp(p.value)}</div>
    {#if p.change}
      <div class="text-sm text-primary mt-0.5">{resolveProp(p.change)}</div>
    {/if}
  </div>

{:else if def.component === 'dot'}
  {@const dotVariant = resolveProp(p.variant)}
  {@const dotColor = dotVariant === 'error' ? 'bg-error'
    : dotVariant === 'warning' ? 'bg-warning'
    : dotVariant === 'success' ? 'bg-success'
    : 'bg-base-content/40'}
  <div class="w-2 h-2 rounded-full shrink-0 {dotColor}"></div>

{:else}
  <!-- Unknown component: {def.component} -->
  {#each staticChildren as child}
    <svelte:self def={child} {rootData} {scopeData} {components} {onaction} />
  {/each}
{/if}

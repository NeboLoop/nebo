<script lang="ts">
  import type { A2UIView, A2UIComponentDef } from './types.js';
  import A2Node from './A2Node.svelte';

  let { view, onaction }: {
    view: A2UIView;
    onaction?: (name: string, payload?: Record<string, unknown>) => void;
  } = $props();

  // Build component Map from flat array for O(1) lookups
  const componentMap = $derived.by(() => {
    const map = new Map<string, A2UIComponentDef>();
    for (const comp of view.components) {
      map.set(comp.id, comp);
    }
    return map;
  });

  // Find root component — the first component whose id is not referenced as a child by any other
  const rootDef = $derived.by(() => {
    const childIds = new Set<string>();
    for (const comp of view.components) {
      if (comp.children) {
        if (Array.isArray(comp.children)) {
          for (const id of comp.children) childIds.add(id);
        } else {
          childIds.add(comp.children.componentId);
        }
      }
    }
    // Root = first component not referenced as a child
    return view.components.find(c => !childIds.has(c.id)) ?? view.components[0];
  });
</script>

{#if rootDef}
  <A2Node
    def={rootDef}
    rootData={view.data}
    components={componentMap}
    {onaction}
  />
{/if}

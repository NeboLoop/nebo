import { writable, derived } from 'svelte/store';

// Per-section collapse state
const collapsedMap = writable<Record<string, boolean>>({});

/**
 * Returns a store-compatible object for the given section's collapse state.
 * Usage: const sidebarCollapsed = sidebarCollapsedFor('agents');
 *        $sidebarCollapsed (read), $sidebarCollapsed = true (write)
 */
export function sidebarCollapsedFor(section: string) {
  const { subscribe } = derived(collapsedMap, ($m) => $m[section] ?? false);
  return {
    subscribe,
    set: (val: boolean) => collapsedMap.update((m) => ({ ...m, [section]: val })),
    update: (fn: (val: boolean) => boolean) =>
      collapsedMap.update((m) => ({ ...m, [section]: fn(m[section] ?? false) })),
  };
}

// Legacy default — keep backward compat for any import that hasn't been updated yet
export const sidebarCollapsed = sidebarCollapsedFor('agents');

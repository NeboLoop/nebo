import { writable, derived } from 'svelte/store';

// Per-section collapse state, persisted to localStorage so it survives reloads.
const STORAGE_KEY = 'nebo:sidebar-collapsed';

function loadCollapsed(): Record<string, boolean> {
  if (typeof localStorage === 'undefined') return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

const collapsedMap = writable<Record<string, boolean>>(loadCollapsed());

// Persist on every change (browser only; no-op during SSR).
if (typeof localStorage !== 'undefined') {
  collapsedMap.subscribe((m) => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(m));
    } catch {
      // ignore quota / privacy-mode failures
    }
  });
}

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

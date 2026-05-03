import { writable, derived } from 'svelte/store';

type ItemType = 'skill' | 'agent' | 'plugin' | 'connector';
interface InstalledItem { id: string; name: string; type: ItemType; installed: string }
interface Dependency { id: string; name: string }

export const installedItems = writable<InstalledItem[]>([]);

let loaded = false;

/** Load installed items from backend API */
export async function loadInstalledItems(): Promise<void> {
  if (loaded) return;
  try {
    const api = await import('$lib/api/nebo');
    const [plugins, tools] = await Promise.all([
      api.listPlugins().catch(() => null),
      api.listTools().catch(() => null),
    ]);
    const items: InstalledItem[] = [];
    if (plugins?.plugins?.length) {
      for (const p of plugins.plugins) {
        items.push({ id: p.id || p.slug, name: p.name, type: 'plugin', installed: '' });
      }
    }
    if (tools?.tools?.length) {
      for (const t of tools.tools) {
        items.push({ id: t.id || t.name, name: t.name, type: 'skill', installed: '' });
      }
    }
    if (items.length) {
      installedItems.set(items);
    }
    loaded = true;
  } catch { /* keep mock data */ }
}

// Derived set of installed IDs for quick lookup
export const installedIds = derived(installedItems, ($items) =>
  new Set($items.map(i => i.id))
);

// Install an item — if it's an agent, auto-install its required skills and plugins
export function installItem(item: { id: string; name: string; type: ItemType }) {
  installedItems.update(items => {
    const existing = new Set(items.map(i => i.id));
    const toAdd: InstalledItem[] = [];

    if (!existing.has(item.id)) {
      toAdd.push({ ...item, installed: 'Just now' });
      existing.add(item.id);
    }

    // Cascade: auto-install agent dependencies (backend handles dependency resolution)

    return toAdd.length ? [...items, ...toAdd] : items;
  });
  // Fire-and-forget API call
  import('$lib/api/nebo').then(api => api.installStoreProduct(item.id)).catch(() => {});
}

export function uninstallItem(id: string) {
  installedItems.update(items => items.filter(i => i.id !== id));
  // Fire-and-forget API call
  import('$lib/api/nebo').then(api => api.uninstallStoreProduct(id)).catch(() => {});
}
